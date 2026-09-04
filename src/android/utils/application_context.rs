use crate::core::runtime::LinuxRuntime;
use crate::{
    android::utils::ndk::run_in_jvm,
    core::config::{parse_config, LocalConfig, ARCH_FS_ROOT, CONFIG_FILE},
};
use jni::{
    objects::{JObject, JString},
    JNIEnv, JavaVM,
};
use std::path::PathBuf;
use std::sync::RwLock;
use winit::platform::android::activity::AndroidApp;

#[derive(Clone)]
pub struct ApplicationContext {
    pub cache_dir: PathBuf,
    pub data_dir: PathBuf,
    pub native_library_dir: PathBuf,
    pub local_config: LocalConfig,
    pub permission_all_files_access: bool,
    pub android_app: AndroidApp,
}

impl std::fmt::Debug for ApplicationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplicationContext")
            .field("cache_dir", &self.cache_dir)
            .field("data_dir", &self.data_dir)
            .field("native_library_dir", &self.native_library_dir)
            .field("local_config", &self.local_config)
            .field(
                "permission_all_files_access",
                &self.permission_all_files_access,
            )
            .finish()
    }
}

impl ApplicationContext {
    pub fn build(android_app: &AndroidApp) {
        let vm = unsafe { JavaVM::from_raw(android_app.vm_as_ptr() as *mut _) }
            .expect("Failed to get JavaVM");
        let mut env = vm
            .attach_current_thread()
            .expect("Failed to attach current thread");

        let activity = unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _) };

        let cache_dir = Self::get_path(&mut env, &activity, "getCacheDir");
        let data_dir = Self::get_path(&mut env, &activity, "getFilesDir");
        let native_library_dir = Self::get_native_library_dir(&mut env, &activity);
        let runtime = crate::android::runtime::proot::PRootRuntime::active();
        let rootfs = runtime.rootfs_path();
        let config_candidate = rootfs.join(CONFIG_FILE.trim_start_matches('/'));
        let full_config_path = if config_candidate.exists() {
            config_candidate.to_string_lossy().to_string()
        } else {
            format!("{}{}", ARCH_FS_ROOT, CONFIG_FILE)
        };
        let local_config = parse_config(full_config_path);
        let permission_all_files_access = Self::is_all_files_access_granted(android_app);

        {
            let mut context = APPLICATION_CONTEXT
                .write()
                .expect("Failed to write application context");
            *context = Some(ApplicationContext {
                cache_dir,
                data_dir,
                native_library_dir,
                local_config,
                permission_all_files_access,
                android_app: android_app.clone(),
            });
            log::info!(
                "ApplicationContext initialized: {:?}",
                context.as_ref().unwrap()
            );
        }
    }

    fn get_path(env: &mut JNIEnv, activity: &JObject, method: &str) -> PathBuf {
        let path_obj = env
            .call_method(activity, method, "()Ljava/io/File;", &[])
            .expect("Failed to call method")
            .l()
            .expect("Failed to get path object");
        let path_str = env
            .call_method(path_obj, "getAbsolutePath", "()Ljava/lang/String;", &[])
            .expect("Failed to get absolute path")
            .l()
            .expect("Failed to get path string");
        let path: String = env
            .get_string(&JString::from(path_str))
            .expect("Failed to convert path to string")
            .into();
        PathBuf::from(path)
    }

    fn get_native_library_dir(env: &mut JNIEnv, activity: &JObject) -> PathBuf {
        let app_info = env
            .call_method(
                activity,
                "getApplicationInfo",
                "()Landroid/content/pm/ApplicationInfo;",
                &[],
            )
            .expect("Failed to get application info")
            .l()
            .expect("Failed to get application info object");
        let native_library_dir = env
            .get_field(app_info, "nativeLibraryDir", "Ljava/lang/String;")
            .expect("Failed to get native library dir field")
            .l()
            .expect("Failed to get native library dir object");
        let path: String = env
            .get_string(&JString::from(native_library_dir))
            .expect("Failed to convert native library dir to string")
            .into();
        PathBuf::from(path)
    }

    fn is_all_files_access_granted(android_app: &AndroidApp) -> bool {
        // To determine whether your app has been granted the MANAGE_EXTERNAL_STORAGE permission, call Environment.isExternalStorageManager().
        // Source: https://developer.android.com/training/data-storage/manage-all-files
        run_in_jvm(
            |env, _| {
                env.call_static_method(
                    "android/os/Environment",
                    "isExternalStorageManager",
                    "()Z",
                    &[],
                )
                .and_then(|value| value.z())
                .unwrap_or(false)
            },
            android_app.clone(),
        )
    }

    /// Query Android ConnectivityManager for active DNS servers associated with the current default network.
    pub fn get_active_dns_servers(&self) -> Vec<String> {
        run_in_jvm(
            |env, app| {
                let mut servers = Vec::new();
                let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _) };

                let Ok(service_name) = env.new_string("connectivity") else {
                    return servers;
                };

                let Ok(cm) = env
                    .call_method(
                        &activity,
                        "getSystemService",
                        "(Ljava/lang/String;)Ljava/lang/Object;",
                        &[(&service_name).into()],
                    )
                    .and_then(|v| v.l())
                else {
                    return servers;
                };

                if cm.is_null() {
                    return servers;
                }

                let Ok(network) = env
                    .call_method(&cm, "getActiveNetwork", "()Landroid/net/Network;", &[])
                    .and_then(|v| v.l())
                else {
                    return servers;
                };

                if network.is_null() {
                    return servers;
                }

                let Ok(link_properties) = env
                    .call_method(
                        &cm,
                        "getLinkProperties",
                        "(Landroid/net/Network;)Landroid/net/LinkProperties;",
                        &[(&network).into()],
                    )
                    .and_then(|v| v.l())
                else {
                    return servers;
                };

                if link_properties.is_null() {
                    return servers;
                }

                let Ok(dns_list) = env
                    .call_method(&link_properties, "getDnsServers", "()Ljava/util/List;", &[])
                    .and_then(|v| v.l())
                else {
                    return servers;
                };

                if dns_list.is_null() {
                    return servers;
                }

                let Ok(size) = env
                    .call_method(&dns_list, "size", "()I", &[])
                    .and_then(|v| v.i())
                else {
                    return servers;
                };

                for i in 0..size {
                    let Ok(inet_addr) = env
                        .call_method(&dns_list, "get", "(I)Ljava/lang/Object;", &[i.into()])
                        .and_then(|v| v.l())
                    else {
                        continue;
                    };

                    if inet_addr.is_null() {
                        continue;
                    }

                    let Ok(host_addr) = env
                        .call_method(&inet_addr, "getHostAddress", "()Ljava/lang/String;", &[])
                        .and_then(|v| v.l())
                    else {
                        continue;
                    };

                    if let Ok(addr_str) = env.get_string(&JString::from(host_addr)) {
                        let ip: String = addr_str.into();
                        let clean_ip = ip.split('%').next().unwrap_or(&ip).trim().to_string();
                        if !clean_ip.is_empty() && !servers.contains(&clean_ip) {
                            servers.push(clean_ip);
                        }
                    }
                }

                servers
            },
            self.android_app.clone(),
        )
    }

    /// Return Android's authoritative IANA timezone identifier.
    ///
    /// PRoot has no timedated service or hardware clock. Mirroring this value
    /// on every launch gives libc and Plasma the same wall-clock zone as the
    /// physical tablet without pretending the guest can change Android time.
    pub fn get_timezone_id(&self) -> Option<String> {
        run_in_jvm(
            |env, _| {
                let zone = env
                    .call_static_method(
                        "java/util/TimeZone",
                        "getDefault",
                        "()Ljava/util/TimeZone;",
                        &[],
                    )
                    .ok()?
                    .l()
                    .ok()?;
                let id = env
                    .call_method(&zone, "getID", "()Ljava/lang/String;", &[])
                    .ok()?
                    .l()
                    .ok()?;
                if id.is_null() {
                    return None;
                }
                env.get_string(&JString::from(id))
                    .ok()
                    .map(|value| value.to_string_lossy().into_owned())
            },
            self.android_app.clone(),
        )
    }
}

static APPLICATION_CONTEXT: RwLock<Option<ApplicationContext>> = RwLock::new(None);
pub fn get_application_context() -> ApplicationContext {
    return APPLICATION_CONTEXT
        .read()
        .expect("Failed to read application context")
        .clone()
        .expect("ApplicationContext is not initialized. Please make sure `ApplicationContext::build(&android_app);` is called in `android_main`.");
}
