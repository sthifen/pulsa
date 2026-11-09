package dev.pulsa;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothClass;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothHidDevice;
import android.bluetooth.BluetoothHidDeviceAppSdpSettings;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.util.Log;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Executors;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * La tablet como teclado y ratón Bluetooth de verdad (perfil HID de
 * dispositivo, Android 9+). El otro equipo no necesita nada instalado: la
 * empareja como cualquier teclado BT (una Raspberry, otro PC, un móvil…).
 *
 * Recibe los mismos mensajes que el servidor del PC ({@code {"t":"k","c":30,"d":true}}…)
 * con códigos evdev, y los traduce a informes HID: teclado (id 1), ratón
 * con rueda (id 2) y teclas multimedia (id 3).
 */
@TargetApi(Build.VERSION_CODES.P)
@SuppressLint("MissingPermission") // los permisos se piden antes de crear esto
final class BtHid {
    private static final String TAG = "PulsaBT";
    private static final byte ID_KEYS = 1, ID_MOUSE = 2, ID_MEDIA = 3;

    /** Descriptor HID: teclado de arranque + ratón con rueda y desplazamiento lateral + multimedia. */
    private static final byte[] DESCRIPTOR = bytes(
            // Teclado
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, ID_KEYS,
            0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
            0x95, 0x01, 0x75, 0x08, 0x81, 0x01,
            0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02,
            0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
            0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00,
            0xC0,
            // Ratón
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, ID_MOUSE, 0x09, 0x01, 0xA1, 0x00,
            0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02,
            0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
            0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06,
            0x05, 0x0C, 0x0A, 0x38, 0x02, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
            0xC0, 0xC0,
            // Multimedia
            0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, ID_MEDIA,
            0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, 0x75, 0x10, 0x95, 0x01, 0x81, 0x00,
            0xC0);

    private static byte[] bytes(int... v) {
        byte[] b = new byte[v.length];
        for (int i = 0; i < v.length; i++) b[i] = (byte) v[i];
        return b;
    }

    // ── Códigos evdev → uso HID ────────────────────────────────────────────

    private static final int[] USAGE = new int[256];
    static {
        int[][] map = {
                {1, 41}, {14, 42}, {15, 43}, {28, 40}, {57, 44}, {12, 45}, {13, 46}, {26, 47}, {27, 48},
                {43, 49}, {39, 51}, {40, 52}, {41, 53}, {51, 54}, {52, 55}, {53, 56}, {58, 57},
                {87, 68}, {88, 69}, {99, 70}, {70, 71}, {119, 72}, {110, 73}, {102, 74}, {104, 75},
                {111, 76}, {107, 77}, {109, 78}, {106, 79}, {105, 80}, {108, 81}, {103, 82}, {69, 83},
                {98, 84}, {55, 85}, {74, 86}, {78, 87}, {96, 88}, {79, 89}, {80, 90}, {81, 91}, {75, 92},
                {76, 93}, {77, 94}, {71, 95}, {72, 96}, {73, 97}, {82, 98}, {83, 99}, {86, 100}, {127, 101},
        };
        for (int[] m : map) USAGE[m[0]] = m[1];
        // Letras: evdev en orden de teclado, HID en orden alfabético.
        int[] letters = {30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44};
        for (int i = 0; i < letters.length; i++) USAGE[letters[i]] = 4 + i;
        for (int i = 0; i < 10; i++) USAGE[2 + i] = 30 + i;   // 1…9, 0
        for (int i = 0; i < 10; i++) USAGE[59 + i] = 58 + i;  // F1…F10
    }

    /** Bit de modificador del informe; -1 si no es modificador. */
    private static int modifierBit(int code) {
        switch (code) {
            case 29: return 0;   // Ctrl izq.
            case 42: return 1;   // Mayús izq.
            case 56: return 2;   // Alt
            case 125: return 3;  // Super izq.
            case 97: return 4;   // Ctrl der.
            case 54: return 5;   // Mayús der.
            case 100: return 6;  // AltGr
            case 126: return 7;  // Super der.
            default: return -1;
        }
    }

    private static int mediaUsage(int code) {
        switch (code) {
            case 113: return 0xE2; // silenciar
            case 114: return 0xEA; // bajar volumen
            case 115: return 0xE9; // subir volumen
            case 164: return 0xCD; // reproducir/pausa
            case 163: return 0xB5; // siguiente
            case 165: return 0xB6; // anterior
            default: return 0;
        }
    }

    // ── Estado ─────────────────────────────────────────────────────────────

    private final Context ctx;
    private final BluetoothAdapter adapter;
    private BluetoothHidDevice hid;
    private BluetoothDevice host;
    private volatile String state = "starting"; // starting, ready, connecting, connected, error
    private volatile String error = "";
    private volatile boolean caps, num;

    private int modifiers;
    private final List<Integer> keys = new ArrayList<>();
    private int buttons;
    private double restX, restY;
    private int restWheel, restPan;
    private int media;

    BtHid(Context ctx) {
        this.ctx = ctx.getApplicationContext();
        BluetoothManager bm = (BluetoothManager) ctx.getSystemService(Context.BLUETOOTH_SERVICE);
        adapter = bm != null ? bm.getAdapter() : null;
        if (adapter == null) {
            fail("Esta tablet no tiene Bluetooth.");
            return;
        }
        if (!adapter.isEnabled()) {
            fail("Activa el Bluetooth de la tablet.");
            return;
        }
        boolean ok = adapter.getProfileProxy(this.ctx, new BluetoothProfile.ServiceListener() {
            @Override
            public void onServiceConnected(int profile, BluetoothProfile proxy) {
                hid = (BluetoothHidDevice) proxy;
                register();
            }

            @Override
            public void onServiceDisconnected(int profile) {
                hid = null;
                fail("El Bluetooth se detuvo.");
            }
        }, BluetoothProfile.HID_DEVICE);
        if (!ok) fail("Esta tablet no permite hacer de teclado Bluetooth.");
    }

    private void fail(String why) {
        Log.w(TAG, why);
        error = why;
        state = "error";
    }

    private SharedPreferences prefs() {
        return ctx.getSharedPreferences("pulsa", Context.MODE_PRIVATE);
    }

    private void register() {
        BluetoothHidDeviceAppSdpSettings sdp = new BluetoothHidDeviceAppSdpSettings(
                "Pulsa", "Teclado y trackpad", "Pulsa",
                BluetoothHidDevice.SUBCLASS1_COMBO, DESCRIPTOR);
        boolean ok = hid.registerApp(sdp, null, null, Executors.newSingleThreadExecutor(), new BluetoothHidDevice.Callback() {
            @Override
            public void onAppStatusChanged(BluetoothDevice device, boolean registered) {
                Log.i(TAG, "registrado: " + registered + " " + device);
                if (!registered) {
                    if (!"connected".equals(state)) fail("Otra app está usando el teclado Bluetooth de la tablet.");
                    return;
                }
                state = "ready";
                error = "";
                // Volver con el último equipo, si sigue emparejado.
                // Android pasa aquí el último equipo conectado; si no, el guardado.
                BluetoothDevice target = device;
                String last = prefs().getString("bt_last", "");
                if (target == null && !last.isEmpty()) {
                    for (BluetoothDevice d : adapter.getBondedDevices()) {
                        if (d.getAddress().equals(last)) target = d;
                    }
                }
                if (target != null && hid.getConnectionState(target) != BluetoothProfile.STATE_CONNECTED) {
                    state = "connecting";
                    hid.connect(target);
                }
            }

            @Override
            public void onConnectionStateChanged(BluetoothDevice device, int st) {
                Log.i(TAG, "conexión " + device + ": " + st);
                if (st == BluetoothProfile.STATE_CONNECTED) {
                    host = device;
                    state = "connected";
                    prefs().edit().putString("bt_last", device.getAddress()).apply();
                } else if (st == BluetoothProfile.STATE_CONNECTING) {
                    state = "connecting";
                } else if (st == BluetoothProfile.STATE_DISCONNECTED) {
                    if (device.equals(host)) {
                        host = null;
                        clear();
                    }
                    // También si falló un intento: no quedarse en "conectando".
                    if (host == null) state = "ready";
                }
            }

            @Override
            public void onGetReport(BluetoothDevice device, byte type, byte id, int bufferSize) {
                byte[] r = id == ID_MOUSE ? new byte[5] : id == ID_MEDIA ? new byte[2] : keyReport();
                hid.replyReport(device, type, id == 0 ? ID_KEYS : id, r);
            }

            @Override
            public void onSetReport(BluetoothDevice device, byte type, byte id, byte[] data) {
                leds(data);
                hid.reportError(device, BluetoothHidDevice.ERROR_RSP_SUCCESS);
            }

            @Override
            public void onInterruptData(BluetoothDevice device, byte id, byte[] data) {
                leds(data);
            }
        });
        if (!ok) fail("No se pudo registrar el teclado Bluetooth.");
    }

    /** El otro equipo enciende Bloq Mayús / Bloq Num con un informe de salida. */
    private void leds(byte[] data) {
        if (data == null || data.length == 0) return;
        int b = data[data.length - 1];
        num = (b & 1) != 0;
        caps = (b & 2) != 0;
    }

    // ── Lo que llama la página ─────────────────────────────────────────────

    synchronized void handle(String json) {
        if (hid == null || host == null) return;
        try {
            JSONObject m = new JSONObject(json);
            switch (m.optString("t")) {
                case "k": key(m.optInt("c"), m.optBoolean("d")); break;
                case "b": button(m.optInt("b"), m.optBoolean("d")); break;
                case "m": move(m.optDouble("x", 0), m.optDouble("y", 0)); break;
                case "s": scroll(m.optInt("x"), m.optInt("y")); break;
                default: break;
            }
        } catch (JSONException e) {
            Log.w(TAG, "mensaje no válido: " + json);
        }
    }

    private void key(int code, boolean down) {
        int bit = modifierBit(code);
        int mediaUse = mediaUsage(code);
        if (bit >= 0) {
            modifiers = down ? modifiers | (1 << bit) : modifiers & ~(1 << bit);
        } else if (mediaUse != 0) {
            media = down ? mediaUse : 0;
            hid.sendReport(host, ID_MEDIA, new byte[]{(byte) media, (byte) (media >> 8)});
            return;
        } else if (code > 0 && code < USAGE.length && USAGE[code] != 0) {
            Integer u = USAGE[code];
            if (down) {
                if (!keys.contains(u) && keys.size() < 6) keys.add(u);
            } else {
                keys.remove(u);
            }
        } else {
            return;
        }
        hid.sendReport(host, ID_KEYS, keyReport());
    }

    private byte[] keyReport() {
        byte[] r = new byte[8];
        r[0] = (byte) modifiers;
        for (int i = 0; i < keys.size(); i++) r[2 + i] = (byte) (int) keys.get(i);
        return r;
    }

    private void button(int b, boolean down) {
        int bit = b == 2 ? 2 : b == 1 ? 4 : 1;
        buttons = down ? buttons | bit : buttons & ~bit;
        mouse(0, 0, 0, 0);
    }

    private void move(double x, double y) {
        restX += x;
        restY += y;
        int ix = (int) restX, iy = (int) restY;
        restX -= ix;
        restY -= iy;
        // Un informe lleva como mucho ±127 por eje.
        while (ix != 0 || iy != 0) {
            int sx = Math.max(-127, Math.min(127, ix)), sy = Math.max(-127, Math.min(127, iy));
            mouse(sx, sy, 0, 0);
            ix -= sx;
            iy -= sy;
        }
    }

    /** En unidades de 120 por muesca, como el servidor del PC. */
    private void scroll(int x, int y) {
        restWheel += y;
        restPan += x;
        int w = restWheel / 120, p = restPan / 120;
        restWheel -= w * 120;
        restPan -= p * 120;
        if (w != 0 || p != 0) mouse(0, 0, w, p);
    }

    private void mouse(int x, int y, int wheel, int pan) {
        hid.sendReport(host, ID_MOUSE, new byte[]{(byte) buttons, (byte) x, (byte) y,
                (byte) Math.max(-127, Math.min(127, wheel)), (byte) Math.max(-127, Math.min(127, pan))});
    }

    /** Suelta todo (la página se fue o el equipo se desconectó). */
    private void clear() {
        modifiers = 0;
        keys.clear();
        buttons = 0;
        media = 0;
    }

    synchronized void releaseAll() {
        clear();
        if (hid != null && host != null) {
            hid.sendReport(host, ID_KEYS, keyReport());
            mouse(0, 0, 0, 0);
            hid.sendReport(host, ID_MEDIA, new byte[2]);
        }
    }

    String state() {
        try {
            JSONObject o = new JSONObject().put("state", state).put("error", error)
                    .put("caps", caps).put("num", num);
            if (host != null) o.put("device", name(host));
            return o.toString();
        } catch (JSONException e) {
            return "{\"state\":\"error\"}";
        }
    }

    /** Equipos emparejados, para conectarse a uno sin esperar a que él lo haga. */
    String devices() {
        JSONArray a = new JSONArray();
        if (adapter == null || !adapter.isEnabled()) return a.toString();
        for (BluetoothDevice d : adapter.getBondedDevices()) {
            if (!canHostKeyboard(d) && !d.equals(host)) continue;
            try {
                a.put(new JSONObject().put("address", d.getAddress()).put("name", name(d))
                        .put("connected", d.equals(host)));
            } catch (JSONException ignored) {
            }
        }
        return a.toString();
    }

    /**
     * Auriculares, altavoces o relojes nunca aceptan un teclado: se dejan
     * fuera. Se quedan ordenadores, móviles y lo que no dice qué es (algunas
     * Raspberry y adaptadores USB se anuncian así).
     */
    private static boolean canHostKeyboard(BluetoothDevice d) {
        BluetoothClass c = d.getBluetoothClass();
        if (c == null) return true;
        int major = c.getMajorDeviceClass();
        return major == BluetoothClass.Device.Major.COMPUTER
                || major == BluetoothClass.Device.Major.PHONE
                || major == BluetoothClass.Device.Major.UNCATEGORIZED
                || major == BluetoothClass.Device.Major.MISC;
    }

    void connect(String address) {
        if (hid == null || adapter == null) return;
        for (BluetoothDevice d : adapter.getBondedDevices()) {
            if (d.getAddress().equals(address)) {
                if (host != null && !host.equals(d)) hid.disconnect(host);
                state = "connecting";
                hid.connect(d);
            }
        }
    }

    private static String name(BluetoothDevice d) {
        String n = d.getName();
        return n == null || n.isEmpty() ? d.getAddress() : n;
    }

    void close() {
        releaseAll();
        if (hid != null) {
            if (host != null) hid.disconnect(host);
            hid.unregisterApp();
            adapter.closeProfileProxy(BluetoothProfile.HID_DEVICE, hid);
            hid = null;
        }
    }
}
