//! The link between the PipeWire sink process and the Android stream.
//!
//! The sink is a helper process, but the Android stream has to belong to the
//! app's own process: Android lists what is playing by process, and a helper
//! is not one of the app's processes to it, so per-app volume controls would
//! not show Portal. The sink therefore sends each quantum over a socket to
//! the app, which plays it and answers when Android has taken it. That answer
//! is what paces the sink.
//!
//! Requests are `[op u32][samples u32]` and, for `PLAY`, that many `f32`
//! samples (interleaved). Each request gets one reply, an `i64`: the output
//! latency in nanoseconds, or `LATENCY_UNKNOWN`. Everything is in native byte
//! order, as both ends are the same build on the same device. The app closes
//! the link when it cannot play any more; the sink exits and is restarted.

use std::io::{self, Read, Write};

const OP_PLAY: u32 = 1;
const OP_STOP: u32 = 2;

/// More than any quantum the sink accepts (8192 frames of 8 channels).
pub const MAX_SAMPLES: usize = 8192 * 8;
pub const LATENCY_UNKNOWN: i64 = -1;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Request {
    /// Play the samples, starting the stream if it is stopped.
    Play,
    /// Stop the stream: only silence follows.
    Stop,
}

fn sample_bytes(samples: &[f32]) -> &[u8] {
    // SAFETY: any f32 is four initialised bytes, and u8 has no alignment.
    unsafe { std::slice::from_raw_parts(samples.as_ptr().cast(), std::mem::size_of_val(samples)) }
}

fn sample_bytes_mut(samples: &mut [f32]) -> &mut [u8] {
    // SAFETY: as above, and every bit pattern is a valid f32.
    unsafe {
        std::slice::from_raw_parts_mut(samples.as_mut_ptr().cast(), std::mem::size_of_val(samples))
    }
}

fn send_header(link: &mut impl Write, op: u32, samples: usize) -> io::Result<()> {
    let mut header = [0u8; 8];
    header[..4].copy_from_slice(&op.to_ne_bytes());
    header[4..].copy_from_slice(&(samples as u32).to_ne_bytes());
    link.write_all(&header)
}

pub fn send_play(link: &mut impl Write, samples: &[f32]) -> io::Result<()> {
    if samples.len() > MAX_SAMPLES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "quantum too long",
        ));
    }
    send_header(link, OP_PLAY, samples.len())?;
    link.write_all(sample_bytes(samples))
}

pub fn send_stop(link: &mut impl Write) -> io::Result<()> {
    send_header(link, OP_STOP, 0)
}

/// Read the next request, with a `Play`'s samples left in `samples`. `None`
/// when the other end has closed the link.
pub fn receive(link: &mut impl Read, samples: &mut Vec<f32>) -> io::Result<Option<Request>> {
    let mut header = [0u8; 8];
    match link.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let op = u32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
    let count = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]) as usize;
    samples.clear();
    match (op, count) {
        (OP_STOP, 0) => Ok(Some(Request::Stop)),
        (OP_PLAY, 1..=MAX_SAMPLES) => {
            samples.resize(count, 0.0);
            link.read_exact(sample_bytes_mut(samples))?;
            Ok(Some(Request::Play))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad audio request op={op} samples={count}"),
        )),
    }
}

pub fn send_reply(link: &mut impl Write, latency_ns: i64) -> io::Result<()> {
    link.write_all(&latency_ns.to_ne_bytes())
}

pub fn receive_reply(link: &mut impl Read) -> io::Result<i64> {
    let mut reply = [0u8; 8];
    link.read_exact(&mut reply)?;
    Ok(i64::from_ne_bytes(reply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_round_trip() {
        let mut wire = Vec::new();
        send_play(&mut wire, &[0.5, -0.25, 1.0, 0.0]).unwrap();
        send_stop(&mut wire).unwrap();
        send_reply(&mut wire, 220_000_000).unwrap();
        send_reply(&mut wire, LATENCY_UNKNOWN).unwrap();

        let mut wire = wire.as_slice();
        let mut samples = vec![9.0];
        assert_eq!(
            receive(&mut wire, &mut samples).unwrap(),
            Some(Request::Play)
        );
        assert_eq!(samples, [0.5, -0.25, 1.0, 0.0]);
        assert_eq!(
            receive(&mut wire, &mut samples).unwrap(),
            Some(Request::Stop)
        );
        assert!(samples.is_empty());
        assert_eq!(receive_reply(&mut wire).unwrap(), 220_000_000);
        assert_eq!(receive_reply(&mut wire).unwrap(), LATENCY_UNKNOWN);
        // The other end is gone.
        assert_eq!(receive(&mut wire, &mut samples).unwrap(), None);
    }

    #[test]
    fn a_bad_request_is_an_error() {
        let mut samples = Vec::new();
        let oversized = [
            OP_PLAY.to_ne_bytes(),
            (MAX_SAMPLES as u32 + 1).to_ne_bytes(),
        ]
        .concat();
        assert!(receive(&mut oversized.as_slice(), &mut samples).is_err());
        let unknown = [7u32.to_ne_bytes(), 0u32.to_ne_bytes()].concat();
        assert!(receive(&mut unknown.as_slice(), &mut samples).is_err());
        assert!(send_play(&mut Vec::new(), &vec![0.0; MAX_SAMPLES + 1]).is_err());
    }
}
