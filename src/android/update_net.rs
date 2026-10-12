//! HTTP for Portal's updater: GitHub API reads, HEAD probes and verified
//! downloads.
//!
//! A download lands in `<name>.part`, resumes from it with a Range request
//! when the server allows, and is renamed into place only after its size and
//! SHA-256 match exactly. A connection that drops is waited out for a while;
//! anything else (a wrong checksum, an HTTP error) counts against a small
//! number of attempts.

use crate::core::{
    config::VERSION,
    provisioning::{offline_backoff, tag_network_error, WaitingForNetwork, DOWNLOAD_STALL_TIMEOUT},
    update_sources::Download,
};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const HARD_ATTEMPTS: u32 = 3;

fn user_agent() -> String {
    format!("Portal/{VERSION} (+https://github.com/Retrorerr/Portal)")
}

pub fn client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_STALL_TIMEOUT)
        .tcp_keepalive(Duration::from_secs(15))
        .user_agent(user_agent())
        .build()
        .map_err(|error| anyhow::Error::new(error).context("could not create an HTTP client"))
}

fn check_status(response: reqwest::blocking::Response, url: &str) -> anyhow::Result<reqwest::blocking::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let rate_limited = status == reqwest::StatusCode::FORBIDDEN
        && response
            .headers()
            .get("x-ratelimit-remaining")
            .is_some_and(|remaining| remaining == "0");
    if rate_limited || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("GitHub's rate limit was reached; Portal checks again later");
    }
    // Keep reqwest's error in the chain so 5xx counts as transient.
    let error = response.error_for_status().err();
    match error {
        Some(error) => Err(anyhow::Error::new(error).context(format!("{url} answered {status}"))),
        None => anyhow::bail!("{url} answered {status}"),
    }
}

/// GET a JSON document (GitHub's API asks for its media type and version).
pub fn get_json<T: DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    let response = client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(|error| anyhow::Error::new(error).context(format!("could not reach {url}")))?;
    let response = check_status(response, url)?;
    let text = response
        .text()
        .map_err(|error| anyhow::Error::new(error).context(format!("could not read {url}")))?;
    serde_json::from_str(&text).map_err(|error| anyhow::anyhow!("{url} sent unreadable JSON: {error}"))
}

/// GET a small text document, refusing anything over `limit` bytes.
pub fn get_text(url: &str, limit: usize) -> anyhow::Result<String> {
    let response = client()?
        .get(url)
        .send()
        .map_err(|error| anyhow::Error::new(error).context(format!("could not reach {url}")))?;
    let mut response = check_status(response, url)?;
    let mut body = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| anyhow::Error::new(error).context(format!("could not read {url}")))?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
        anyhow::ensure!(body.len() <= limit, "{url} is larger than expected");
    }
    String::from_utf8(body).map_err(|_| anyhow::anyhow!("{url} is not text"))
}

/// The ETag and Last-Modified a server reports for `url`, following
/// redirects.
pub fn head(url: &str) -> anyhow::Result<(Option<String>, Option<String>)> {
    let response = client()?
        .head(url)
        .send()
        .map_err(|error| anyhow::Error::new(error).context(format!("could not reach {url}")))?;
    let response = check_status(response, url)?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_owned())
    };
    Ok((header("etag"), header("last-modified")))
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Whether `path` holds exactly the bytes `download` describes.
pub fn file_matches(path: &Path, download: &Download) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.len() == download.size)
        && sha256_of(path).is_ok_and(|digest| digest == download.sha256)
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// One attempt: resume or start `<dest>.part`, then verify and rename.
fn download_once(
    download: &Download,
    dest: &Path,
    progress: &mut impl FnMut(u64, u64),
) -> anyhow::Result<()> {
    let part = part_path(dest);
    let mut hasher = Sha256::new();
    let mut have = match fs::metadata(&part) {
        Ok(meta) if meta.len() <= download.size => meta.len(),
        Ok(_) => {
            fs::remove_file(&part)?;
            0
        }
        Err(_) => 0,
    };
    if have > 0 {
        // Hash what is already there so the final digest covers the file.
        let mut file = fs::File::open(&part)?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut left = have;
        while left > 0 {
            let read = file.read(&mut buffer[..left.min(256 * 1024) as usize])?;
            anyhow::ensure!(read > 0, "partial download shrank while reading");
            hasher.update(&buffer[..read]);
            left -= read as u64;
        }
    }

    let mut request = client()?.get(&download.url);
    if have > 0 && have < download.size {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let mut response = if have == download.size {
        None
    } else {
        let response = request
            .send()
            .map_err(|error| anyhow::Error::new(error).context("download failed"))?;
        if have > 0 && response.status() == reqwest::StatusCode::OK {
            // The server ignored the range: start over.
            have = 0;
            hasher = Sha256::new();
            fs::remove_file(&part)?;
        }
        Some(check_status(response, &download.url)?)
    };

    if let Some(response) = response.as_mut() {
        let mut out = OpenOptions::new().create(true).append(true).open(&part)?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut last_report = Instant::now() - Duration::from_secs(1);
        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|error| anyhow::Error::new(error).context("download interrupted"))?;
            if read == 0 {
                break;
            }
            have += read as u64;
            anyhow::ensure!(
                have <= download.size,
                "the download is larger than the release says ({have} > {} bytes)",
                download.size
            );
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
            if last_report.elapsed() >= Duration::from_millis(250) {
                progress(have, download.size);
                last_report = Instant::now();
            }
        }
        out.flush()?;
        let _ = out.sync_all();
    }
    progress(have, download.size);

    if have != download.size {
        // A short body is a dropped connection: keep the part to resume.
        return Err(anyhow::Error::new(std::io::Error::from(
            std::io::ErrorKind::UnexpectedEof,
        ))
        .context(format!("the download stopped at {have} of {} bytes", download.size)));
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != download.sha256 {
        let _ = fs::remove_file(&part);
        anyhow::bail!("the download does not match its SHA-256 checksum");
    }
    fs::rename(&part, dest)?;
    Ok(())
}

/// Download `download` to `dest`, verified. Waits out a lost connection for
/// up to `offline_budget`, then gives up; other failures get
/// [`HARD_ATTEMPTS`] tries. Returns at once when `dest` is already right.
pub fn download_verified(
    download: &Download,
    dest: &Path,
    offline_budget: Duration,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<()> {
    if file_matches(dest, download) {
        progress(download.size, download.size);
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(dest);
    let started = Instant::now();
    let mut failures = 0;
    let mut offline_waits = 0;
    loop {
        let error = match download_once(download, dest, &mut progress) {
            Ok(()) => return Ok(()),
            Err(error) => tag_network_error(error),
        };
        if error.downcast_ref::<WaitingForNetwork>().is_some() && started.elapsed() < offline_budget {
            offline_waits += 1;
            log::warn!("update download waiting for the network: {error:#}");
            std::thread::sleep(offline_backoff(offline_waits));
            continue;
        }
        failures += 1;
        if failures >= HARD_ATTEMPTS || error.downcast_ref::<WaitingForNetwork>().is_some() {
            return Err(error);
        }
        log::warn!("update download attempt {failures} failed: {error:#}");
        std::thread::sleep(Duration::from_secs(2));
    }
}
