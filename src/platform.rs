//! Android platform services — notifications, image picker, text-to-speech,
//! haptics, screen flags — invoked through the activity's Java helper
//! (`GpuiActivity`) so every framework class resolves through the app's own
//! classloader. Raw JNI from NativeActivity threads uses the system loader,
//! and method lookups on freshly constructed framework objects (e.g.
//! `Notification.Builder`) failed on device. Desktop builds get no-ops.

#[cfg(target_os = "android")]
use jni::objects::JValue;

/// Post a notification. `image` attaches a big picture (path to a decodable
/// file — e.g. a blob's final path in mdrv-db — shown expanded in the shade).
pub fn notify(title: &str, body: &str, image: Option<&std::path::Path>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            use jni::objects::{JObject, JString};
            let activity = gpui_mobile::android::jni::activity(env)?;
            let jtitle = env.new_string(title).map_err(|e| e.to_string())?;
            let jbody = env.new_string(body).map_err(|e| e.to_string())?;
            let jimage: JObject = match image {
                Some(p) => {
                    let s: JString = env
                        .new_string(p.to_string_lossy().as_ref())
                        .map_err(|e| e.to_string())?;
                    s.into()
                }
                None => JObject::null(),
            };
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("showNotification"),
                    jni::jni_sig!(
                        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"
                    ),
                    &[
                        JValue::Object(&jtitle),
                        JValue::Object(&jbody),
                        JValue::Object(&jimage),
                    ],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (title, body, image);
        Ok(())
    }
}

/// Launch the system image picker (Android). The result is copied into app
/// storage; poll [`take_picked_image`] until it returns a path.
pub fn pick_image() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("pickImage"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("picker is android-only".into())
    }
}

/// Consume the last picked image path (None until a pick completes).
pub fn take_picked_image() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("takePickedImage"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(None)
            } else {
                Ok(Some(gpui_mobile::android::jni::get_string(env, &res)))
            }
        })
        .ok()
        .flatten()
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Speak `text` via the Android TTS engine (queued after pending speech).
pub fn speak(text: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let jtext = env.new_string(text).map_err(|e| e.to_string())?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("speak"),
                    jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                    &[JValue::Object(&jtext)],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = text;
        Ok(())
    }
}

/// Stop ongoing TTS speech.
pub fn stop_speak() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("stopSpeak"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(())
    }
}

/// Play a bundled UI sound by name ("chime"). Uses the Java SoundPool with
/// sonification usage, which MIUI's audio hardening does not mute (plain
/// media-stream playback from this app is muted as "background playback").
pub fn play_sfx(name: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let jname = env.new_string(name).map_err(|e| e.to_string())?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("playSfx"),
                    jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                    &[JValue::Object(&jname)],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        Ok(())
    }
}

/// Short vibration for tap feedback.
pub fn haptic() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("haptic"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(())
    }
}

/// Keep the screen on (kid-friendly story mode) while the flag is set.
pub fn keep_screen_on(on: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("setKeepScreenOn"),
                    jni::jni_sig!("(Z)Ljava/lang/String;"),
                    &[JValue::Bool(on)],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = on;
        Ok(())
    }
}

/// Immersive sticky fullscreen (hides status + navigation bars).
pub fn immersive(on: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        gpui_mobile::android::jni::with_env(|env| {
            let activity = gpui_mobile::android::jni::activity(env)?;
            let res = env
                .call_method(
                    &activity,
                    jni::jni_str!("setImmersiveMode"),
                    jni::jni_sig!("(Z)Ljava/lang/String;"),
                    &[JValue::Bool(on)],
                )
                .and_then(|v| v.l())
                .map_err(|e| e.to_string())?;
            if res.is_null() {
                Ok(())
            } else {
                Err(gpui_mobile::android::jni::get_string(env, &res))
            }
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = on;
        Ok(())
    }
}
