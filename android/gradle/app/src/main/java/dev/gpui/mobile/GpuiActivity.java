package dev.gpui.mobile;

import android.app.NativeActivity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.media.AudioAttributes;
import android.media.AudioManager;
import android.media.SoundPool;
import android.net.Uri;
import android.os.Bundle;
import android.speech.tts.TextToSpeech;
import android.util.Log;
import android.view.HapticFeedbackConstants;
import android.view.KeyEvent;
import android.view.WindowManager;

import java.util.Locale;

import androidx.core.splashscreen.SplashScreen;

import id.mdrv.lab.R;

/**
 * Custom Activity extending NativeActivity that integrates with the
 * AndroidX SplashScreen API.
 *
 * On API 31+ the system splash screen is displayed automatically via theme
 * attributes. On API 26-30 the AndroidX compat library emulates the same
 * behavior using the theme's windowBackground drawable.
 *
 * The splash screen is held visible until the Rust native library signals
 * that initialization is complete by setting NATIVE_INITIALIZED to true
 * (see src/android/jni.rs). This prevents the user from seeing an empty
 * or partially-rendered surface during startup.
 *
 * Also handles:
 * - Deep link intents (onNewIntent)
 * - Volume key routing to the MUSIC audio stream
 * - Media button events via MediaSessionCompat
 */
public class GpuiActivity extends NativeActivity {

    /** Whether the native .so has been loaded via System.loadLibrary. */
    private static volatile boolean sNativeLibLoaded = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Install the splash screen BEFORE calling super.onCreate().
        // This is required by the AndroidX SplashScreen API.
        SplashScreen splash = SplashScreen.installSplashScreen(this);

        // NativeActivity loads the .so via dlopen (loadNativeCode), which does
        // NOT register JNI symbols with the classloader. We must call
        // System.loadLibrary() ourselves so that JNI can resolve our native
        // methods. Reading the library name from the manifest meta-data ensures
        // we stay in sync with the nativeLibraryName placeholder.
        if (!sNativeLibLoaded) {
            try {
                ActivityInfo ai = getPackageManager().getActivityInfo(
                        getComponentName(), PackageManager.GET_META_DATA);
                String libName = ai.metaData.getString("android.app.lib_name");
                if (libName != null) {
                    System.loadLibrary(libName);
                    sNativeLibLoaded = true;
                }
            } catch (PackageManager.NameNotFoundException e) {
                // Shouldn't happen — we're querying our own activity.
            } catch (UnsatisfiedLinkError e) {
                // Library may already be loaded by NativeActivity; that's fine.
                sNativeLibLoaded = true;
            }
        }

        // Keep the splash screen visible until the native side signals readiness.
        splash.setKeepOnScreenCondition(() -> !isNativeReady());

        // Route volume keys to the MUSIC stream so they control media volume
        // rather than the ringer/notification volume.
        setVolumeControlStream(AudioManager.STREAM_MUSIC);

        super.onCreate(savedInstanceState);

        // Preload UI sound effects so the first tap plays immediately.
        initSfx();
    }

    /**
     * Check if the native library is fully initialized.
     * Returns false if the .so hasn't been loaded yet or if
     * NATIVE_INITIALIZED hasn't been set to true.
     */
    private boolean isNativeReady() {
        if (!sNativeLibLoaded) {
            return false;
        }
        try {
            return nativeIsInitialized();
        } catch (UnsatisfiedLinkError e) {
            return false;
        }
    }

    /**
     * Intercept key events to handle volume and media buttons.
     *
     * NativeActivity normally forwards ALL key events to the native side,
     * which means volume keys would be consumed by the Rust event loop
     * without actually adjusting the system volume. We intercept them here
     * and let the system handle them instead.
     */
    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        int keyCode = event.getKeyCode();
        switch (keyCode) {
            case KeyEvent.KEYCODE_VOLUME_UP:
            case KeyEvent.KEYCODE_VOLUME_DOWN:
            case KeyEvent.KEYCODE_VOLUME_MUTE:
                // Let the system handle volume keys (adjusts STREAM_MUSIC).
                // Don't pass to NativeActivity's native input handler.
                return super.dispatchKeyEvent(event);

            case KeyEvent.KEYCODE_MEDIA_PLAY:
            case KeyEvent.KEYCODE_MEDIA_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_NEXT:
            case KeyEvent.KEYCODE_MEDIA_PREVIOUS:
            case KeyEvent.KEYCODE_MEDIA_STOP:
            case KeyEvent.KEYCODE_HEADSETHOOK:
                // Route media buttons through the MediaSession.
                // MediaButtonReceiver will dispatch to our session callback.
                return super.dispatchKeyEvent(event);

            default:
                return super.dispatchKeyEvent(event);
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
    }

    /**
     * Handle new intents delivered to this singleTask activity.
     *
     * When the app is already running and a deeplink is opened
     * (e.g. `adb shell am start -d gpui://video_player`), this method
     * receives the new intent. We update the activity's intent and
     * notify the Rust side via JNI.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);

        Uri data = intent.getData();
        if (data != null) {
            String url = data.toString();
            Log.i("GpuiActivity", "onNewIntent deeplink: " + url);
            try {
                nativeOnDeepLink(url);
            } catch (UnsatisfiedLinkError e) {
                Log.w("GpuiActivity", "nativeOnDeepLink not available yet");
            }
        }
    }

    // ── Platform services called from Rust via JNI ───────────────────────
    //
    // All of these live in Java (not raw JNI from Rust) so every framework
    // class resolves through the app's own classloader; each returns null
    // on success or the throwable's toString() so the Rust HUD can show it.

    private TextToSpeech tts;
    private volatile boolean ttsReady = false;
    private volatile String pickedImagePath = null;
    private SoundPool sfxPool;
    private int sfxChimeId = 0;
    private volatile boolean sfxReady = false;

    /**
     * Post a notification on the HIGH-importance "mdrv_lab_h" channel.
     * When imagePath is non-null and decodable, BigPictureStyle is used so
     * the image shows expanded in the shade.
     */
    public String showNotification(String title, String text, String imagePath) {
        try {
            NotificationManager nm =
                    (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
            // Channels keep the first importance they were created with, and
            // delete + recreate with the same id resurrects the old settings.
            nm.deleteNotificationChannel("mdrv_lab");
            NotificationChannel ch = new NotificationChannel(
                    "mdrv_lab_h", "mdrv-lab", NotificationManager.IMPORTANCE_HIGH);
            nm.createNotificationChannel(ch);
            Notification.Builder b = new Notification.Builder(this, "mdrv_lab_h")
                    .setSmallIcon(R.mipmap.ic_launcher)
                    .setContentTitle(title)
                    .setContentText(text);
            if (imagePath != null) {
                Bitmap bmp = BitmapFactory.decodeFile(imagePath);
                if (bmp != null) {
                    b.setStyle(new Notification.BigPictureStyle()
                            .bigPicture(bmp)
                            .bigLargeIcon((android.graphics.Bitmap) null));
                }
            }
            nm.notify(1, b.build());
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /** Launch the system image picker (ACTION_GET_CONTENT, openable images). */
    public String pickImage() {
        try {
            Intent i = new Intent(Intent.ACTION_GET_CONTENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("image/*");
            startActivityIfNeeded(Intent.createChooser(i, "Pick an image"), 7001);
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /**
     * Consume the last picked image path (copied out of the content provider
     * into app storage); null until a pick completes. Rust polls this while
     * a pick is pending.
     */
    public String takePickedImage() {
        String p = pickedImagePath;
        pickedImagePath = null;
        return p;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != 7001 || resultCode != RESULT_OK || data == null
                || data.getData() == null) {
            return;
        }
        try {
            java.io.File dir = new java.io.File(getFilesDir(), "picked");
            dir.mkdirs();
            java.io.File out = new java.io.File(dir,
                    "img_" + System.currentTimeMillis() + ".png");
            java.io.InputStream in = getContentResolver().openInputStream(data.getData());
            java.io.FileOutputStream fos = new java.io.FileOutputStream(out);
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) {
                fos.write(buf, 0, n);
            }
            fos.close();
            in.close();
            pickedImagePath = out.getAbsolutePath();
        } catch (Throwable t) {
            Log.w("GpuiActivity", "onActivityResult copy failed: " + t);
        }
    }

    /** Text-to-speech; utterances queue after any pending speech. */
    public String speak(String text) {
        try {
            if (tts == null) {
                tts = new TextToSpeech(this, status -> {
                    ttsReady = status == TextToSpeech.SUCCESS;
                    if (ttsReady) {
                        tts.setLanguage(Locale.US);
                    }
                });
            }
            if (!ttsReady) {
                return null; // still initialising; the next call will speak
            }
            tts.speak(text, TextToSpeech.QUEUE_ADD, null, "mdrv-lab-" + System.nanoTime());
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    public String stopSpeak() {
        try {
            if (tts != null) {
                tts.stop();
            }
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /**
     * UI sound effects go through SoundPool with sonification usage: MIUI's
     * "audio hardening" mutes plain media-stream playback from apps it deems
     * background (observed live: "background playback would be muted for
     * id.mdrv.lab, level: partial"), but sonification UI sounds are exempt.
     */
    private String initSfx() {
        try {
            if (sfxPool != null) {
                return null;
            }
            sfxPool = new SoundPool.Builder()
                    .setMaxStreams(4)
                    .setAudioAttributes(new AudioAttributes.Builder()
                            // USAGE_MEDIA follows the Media volume slider, like games.
                            // USAGE_ASSISTANCE_SONIFICATION plays on the Ring stream,
                            // which is often muted entirely — that's why the ding
                            // was inaudible even though it "played".
                            .setUsage(AudioAttributes.USAGE_MEDIA)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build())
                    .build();
            sfxPool.setOnLoadCompleteListener((pool, sampleId, status) -> {
                if (sampleId == sfxChimeId && status == 0) {
                    sfxReady = true;
                }
            });
            sfxChimeId = sfxPool.load(this, R.raw.chime, 1);
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /** Play a bundled UI sound by name ("chime"). */
    public String playSfx(String name) {
        try {
            String err = initSfx();
            if (err != null) {
                return err;
            }
            if (!sfxReady) {
                return "sfx still loading, tap again";
            }
            sfxPool.play(sfxChimeId, 1.0f, 1.0f, 1, 0, 1.0f);
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /** Keep the screen on (kid-friendly story mode) while the flag is set. */
    public String setKeepScreenOn(boolean on) {
        try {
            runOnUiThread(() -> {
                if (on) {
                    getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                } else {
                    getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                }
            });
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /** Immersive sticky fullscreen (hides status + navigation bars). */
    public String setImmersiveMode(boolean on) {
        try {
            runOnUiThread(() -> {
                android.view.Window w = getWindow();
                android.view.View decor = w.getDecorView();
                // API 30+: the InsetsController is the reliable way to both hide
                // AND re-show the bars. The legacy setSystemUiVisibility path
                // below stopped restoring bars on this device once sticky
                // immersive had been applied.
                if (android.os.Build.VERSION.SDK_INT >= 30) {
                    android.view.WindowInsetsController c = w.getInsetsController();
                    if (c != null) {
                        int bars = android.view.WindowInsets.Type.statusBars()
                                | android.view.WindowInsets.Type.navigationBars();
                        if (on) {
                            c.hide(bars);
                            c.setSystemBarsBehavior(android.view.WindowInsetsController
                                    .BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                        } else {
                            c.show(bars);
                        }
                    }
                }
                if (on) {
                    decor.setSystemUiVisibility(
                            android.view.View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                                    | android.view.View.SYSTEM_UI_FLAG_FULLSCREEN
                                    | android.view.View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                                    | android.view.View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
                } else {
                    decor.setSystemUiVisibility(
                            android.view.View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
                }
            });
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /** Short haptic tick for tap feedback. */
    public String haptic() {
        try {
            getWindow().getDecorView().performHapticFeedback(
                    HapticFeedbackConstants.VIRTUAL_KEY);
            return null;
        } catch (Throwable t) {
            return t.toString();
        }
    }

    /**
     * JNI bridge to check if the Rust NATIVE_INITIALIZED flag is set.
     */
    private static native boolean nativeIsInitialized();

    /**
     * JNI bridge to notify Rust of an incoming deeplink URL.
     */
    private static native void nativeOnDeepLink(String url);
}
