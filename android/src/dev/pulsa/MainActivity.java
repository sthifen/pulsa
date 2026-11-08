package dev.pulsa;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.os.Build;
import android.os.Bundle;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.view.Display;
import android.view.HapticFeedbackConstants;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.webkit.JavascriptInterface;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;

/**
 * El teclado a pantalla completa. El teclado en sí (teclas, sonidos,
 * trackpad) es la página que sirve el PC; la app pone pantalla completa
 * inmersiva, pantalla siempre encendida, vibración y un lanzador que
 * encuentra el PC por USB (adb reverse → localhost) o por WiFi.
 */
public class MainActivity extends Activity {
    private static final String LAUNCHER = "file:///android_asset/launcher.html";
    private final PcFinder finder = new PcFinder();
    private WebView web;
    private Vibrator vibrator;

    @SuppressLint("SetJavaScriptEnabled")
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            getWindow().getAttributes().layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        }
        preferHighestRefreshRate();
        vibrator = (Vibrator) getSystemService(Context.VIBRATOR_SERVICE);

        web = new WebView(this);
        web.setBackgroundColor(Color.rgb(0x15, 0x17, 0x1c));
        web.setHapticFeedbackEnabled(true);
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setAllowFileAccess(true);
        s.setMediaPlaybackRequiresUserGesture(false);
        s.setCacheMode(WebSettings.LOAD_NO_CACHE);
        s.setSupportZoom(false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            web.setRendererPriorityPolicy(WebView.RENDERER_PRIORITY_IMPORTANT, false);
        }
        web.setWebViewClient(new WebViewClient() {
            @Override
            public void onReceivedError(WebView view, WebResourceRequest req, WebResourceError err) {
                // Si el PC no responde, volver al lanzador, que reintenta solo.
                if (req.isForMainFrame() && !req.getUrl().toString().startsWith("file:")) {
                    view.loadUrl(LAUNCHER + "#error");
                }
            }

            @SuppressWarnings("deprecation")
            @Override
            public void onReceivedError(WebView view, int code, String description, String url) {
                if (url != null && !url.startsWith("file:")) view.loadUrl(LAUNCHER + "#error");
            }
        });
        web.addJavascriptInterface(new Bridge(), "PulsaNative");
        setContentView(web);

        if (state != null) web.restoreState(state);
        else web.loadUrl(LAUNCHER);
    }

    /** Muchas tablets dejan las WebView a 60 Hz si no se pide más. */
    @SuppressWarnings("deprecation")
    private void preferHighestRefreshRate() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return;
        Display d = getWindowManager().getDefaultDisplay();
        Display.Mode best = d.getMode();
        for (Display.Mode m : d.getSupportedModes()) {
            if (m.getPhysicalWidth() == best.getPhysicalWidth()
                    && m.getPhysicalHeight() == best.getPhysicalHeight()
                    && m.getRefreshRate() > best.getRefreshRate()) {
                best = m;
            }
        }
        WindowManager.LayoutParams lp = getWindow().getAttributes();
        lp.preferredDisplayModeId = best.getModeId();
        getWindow().setAttributes(lp);
    }

    private final class Bridge {
        private SharedPreferences prefs() {
            return getSharedPreferences("pulsa", MODE_PRIVATE);
        }

        @JavascriptInterface
        public String deviceName() {
            return Build.MODEL;
        }

        @JavascriptInterface
        public boolean usbWorks() {
            return Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP;
        }

        /** Último PC con el que se conectó ("" si ninguno). Por su id: su IP cambia. */
        @JavascriptInterface
        public String lastPc() {
            return prefs().getString("last_pc", "");
        }

        @JavascriptInterface
        public void rememberPc(String id) {
            prefs().edit().putString("last_pc", id).apply();
        }

        @JavascriptInterface
        public void scan(int port) {
            finder.start(port);
        }

        @JavascriptInterface
        public String scanState() {
            return finder.state();
        }

        @JavascriptInterface
        public void disconnect() {
            prefs().edit().remove("last_pc").apply();
            runOnUiThread(() -> web.loadUrl(LAUNCHER + "#manual"));
        }

        /**
         * Vibración corta al pulsar ("down") y aún más leve al soltar ("up").
         * Los efectos predefinidos (Android 10+) usan el motor háptico como
         * el teclado del sistema; antes, un pulso de unos milisegundos.
         */
        @JavascriptInterface
        @SuppressWarnings("deprecation")
        public void haptic(String kind) {
            boolean up = "up".equals(kind);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && vibrator != null && vibrator.hasVibrator()) {
                vibrator.vibrate(VibrationEffect.createPredefined(
                        up ? VibrationEffect.EFFECT_TICK : VibrationEffect.EFFECT_CLICK));
            } else {
                runOnUiThread(() -> web.performHapticFeedback(up
                        ? HapticFeedbackConstants.CLOCK_TICK
                        : HapticFeedbackConstants.KEYBOARD_TAP,
                        HapticFeedbackConstants.FLAG_IGNORE_GLOBAL_SETTING));
            }
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        web.saveState(out);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) enterImmersive();
    }

    @SuppressWarnings("deprecation")
    private void enterImmersive() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController c = getWindow().getInsetsController();
            if (c != null) {
                c.hide(WindowInsets.Type.systemBars());
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                    View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
        }
    }

    @SuppressWarnings("deprecation")
    @Override
    public void onBackPressed() {
        String url = web.getUrl();
        if (url != null && !url.startsWith(LAUNCHER)) web.loadUrl(LAUNCHER + "#manual");
        else super.onBackPressed();
    }

    @Override
    protected void onPause() {
        super.onPause();
        web.onPause();
    }

    @Override
    protected void onResume() {
        super.onResume();
        web.onResume();
    }

    @Override
    protected void onDestroy() {
        web.destroy();
        super.onDestroy();
    }
}
