use crate::core::guest_procfs::NetworkRoute;
use crate::core::runtime::LinuxRuntime;
use crate::{
    android::utils::ndk::run_in_jvm,
    core::config::{parse_config, LocalConfig, CONFIG_FILE, DESKTOP_USER},
};
use jni::{
    objects::{JObject, JString},
    JNIEnv, JavaVM,
};
use std::net::IpAddr;
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
            format!("{}{}", crate::core::config::rootfs().display(), CONFIG_FILE)
        };
        let mut local_config = parse_config(full_config_path);
        if local_config.user.username != DESKTOP_USER {
            log::warn!(
                "Ignoring legacy graphical username {}; Portal Plasma uses the persistent desktop account",
                local_config.user.username
            );
            local_config.user.username = DESKTOP_USER.to_owned();
        }
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

    /// Read DNS and routes from the same Android default-network snapshot.
    pub fn get_active_network(&self) -> (Vec<String>, Vec<NetworkRoute>) {
        run_in_jvm(
            |env, app| {
                (|| -> jni::errors::Result<_> {
                    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _) };
                    let service_name = env.new_string("connectivity")?;
                    let cm = env
                        .call_method(
                            &activity,
                            "getSystemService",
                            "(Ljava/lang/String;)Ljava/lang/Object;",
                            &[(&service_name).into()],
                        )?
                        .l()?;
                    if cm.is_null() {
                        return Ok(Default::default());
                    }
                    let network = env
                        .call_method(&cm, "getActiveNetwork", "()Landroid/net/Network;", &[])?
                        .l()?;
                    if network.is_null() {
                        return Ok(Default::default());
                    }
                    let link_properties = env
                        .call_method(
                            &cm,
                            "getLinkProperties",
                            "(Landroid/net/Network;)Landroid/net/LinkProperties;",
                            &[(&network).into()],
                        )?
                        .l()?;
                    if link_properties.is_null() {
                        return Ok(Default::default());
                    }
                    let mut servers = Vec::new();
                    for address in
                        link_values(env, &link_properties, "getDnsServers", inet_address)?
                    {
                        let address = address.to_string();
                        if !servers.contains(&address) {
                            servers.push(address);
                        }
                    }
                    let sdk = env
                        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
                        .i()?;
                    let routes = link_values(env, &link_properties, "getRoutes", |env, route| {
                        read_route(env, route, sdk >= 33)
                    })
                    .unwrap_or_default();
                    Ok((servers, routes))
                })()
                .unwrap_or_default()
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

    /// Android's primary language as a BCP 47 tag, e.g. `pt-BR`.
    pub fn get_language_tag(&self) -> Option<String> {
        run_in_jvm(
            |env, _| {
                let locale = env
                    .call_static_method("java/util/Locale", "getDefault", "()Ljava/util/Locale;", &[])
                    .ok()?
                    .l()
                    .ok()?;
                let tag = env
                    .call_method(&locale, "toLanguageTag", "()Ljava/lang/String;", &[])
                    .ok()?
                    .l()
                    .ok()?;
                if tag.is_null() {
                    return None;
                }
                env.get_string(&JString::from(tag))
                    .ok()
                    .map(|value| value.to_string_lossy().into_owned())
            },
            self.android_app.clone(),
        )
    }
}

fn link_values<T>(
    env: &mut JNIEnv,
    properties: &JObject,
    method: &str,
    mut read: impl FnMut(&mut JNIEnv, &JObject) -> jni::errors::Result<Option<T>>,
) -> jni::errors::Result<Vec<T>> {
    let list = env
        .call_method(properties, method, "()Ljava/util/List;", &[])?
        .l()?;
    let mut values = Vec::new();
    let size = env.call_method(&list, "size", "()I", &[])?.i()?;
    for index in 0..size {
        let value = env.with_local_frame(16, |env| -> jni::errors::Result<_> {
            let item = env
                .call_method(&list, "get", "(I)Ljava/lang/Object;", &[index.into()])?
                .l()?;
            read(env, &item)
        })?;
        values.extend(value);
    }
    Ok(values)
}

fn read_string(
    env: &mut JNIEnv,
    object: &JObject,
    method: &str,
) -> jni::errors::Result<Option<String>> {
    let string = env
        .call_method(object, method, "()Ljava/lang/String;", &[])?
        .l()?;
    if string.is_null() {
        return Ok(None);
    }
    Ok(Some(env.get_string(&JString::from(string))?.into()))
}

fn inet_address(env: &mut JNIEnv, address: &JObject) -> jni::errors::Result<Option<IpAddr>> {
    if address.is_null() {
        return Ok(None);
    }
    Ok(read_string(env, address, "getHostAddress")?
        .and_then(|value| value.split('%').next()?.parse().ok()))
}

fn read_route(
    env: &mut JNIEnv,
    route: &JObject,
    typed_routes: bool,
) -> jni::errors::Result<Option<NetworkRoute>> {
    const RTN_UNICAST: i32 = 1;
    if typed_routes && env.call_method(route, "getType", "()I", &[])?.i()? != RTN_UNICAST {
        return Ok(None);
    }
    let Some(interface) = read_string(env, route, "getInterface")? else {
        return Ok(None);
    };
    let dest = env
        .call_method(route, "getDestination", "()Landroid/net/IpPrefix;", &[])?
        .l()?;
    let address = env
        .call_method(&dest, "getAddress", "()Ljava/net/InetAddress;", &[])?
        .l()?;
    let Some(destination) = inet_address(env, &address)? else {
        return Ok(None);
    };
    let prefix = env.call_method(&dest, "getPrefixLength", "()I", &[])?.i()?;
    let Ok(prefix) = u8::try_from(prefix) else {
        return Ok(None);
    };
    let gateway = env
        .call_method(route, "getGateway", "()Ljava/net/InetAddress;", &[])?
        .l()?;
    Ok(Some(NetworkRoute {
        interface,
        destination,
        prefix,
        gateway: inet_address(env, &gateway)?,
    }))
}

static APPLICATION_CONTEXT: RwLock<Option<ApplicationContext>> = RwLock::new(None);
pub fn get_application_context() -> ApplicationContext {
    return APPLICATION_CONTEXT
        .read()
        .expect("Failed to read application context")
        .clone()
        .expect("ApplicationContext is not initialized. Please make sure `ApplicationContext::build(&android_app);` is called in `android_main`.");
}
