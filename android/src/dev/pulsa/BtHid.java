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
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.os.Build;
import android.util.Log;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
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
final class BtHid implements Link {
    private static final String TAG = "PulsaBT";
    private static final byte ID_KEYS = HidReports.ID_KEYS, ID_MOUSE = HidReports.ID_MOUSE, ID_MEDIA = HidReports.ID_MEDIA;

    // ── Estado ─────────────────────────────────────────────────────────────

    private final Context ctx;
    private final BluetoothAdapter adapter;
    private BluetoothHidDevice hid;
    private BluetoothDevice host;
    private volatile String state = "starting"; // starting, ready, connecting, connected, error
    private volatile String error = "";
    private volatile boolean caps, num;

    private final HidReports reports = new HidReports();
    private final HidReports.Sink sink = (id, r) -> {
        BluetoothHidDevice h = hid;
        BluetoothDevice d = host;
        if (h != null && d != null) h.sendReport(d, id, r);
    };

    /** Equipos encontrados al buscar desde la tablet (sin emparejar todavía). */
    private final Map<String, BluetoothDevice> found = new LinkedHashMap<>();
    private volatile boolean scanning;
    /** Al que se está emparejando desde la tablet: al terminar, se conecta. */
    private volatile String pairingWith = "";
    private boolean receiverOn;

    private final BroadcastReceiver receiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            String a = i.getAction();
            if (BluetoothDevice.ACTION_FOUND.equals(a)) {
                BluetoothDevice d = i.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE);
                if (d != null && d.getBondState() != BluetoothDevice.BOND_BONDED && canHostKeyboard(d)) {
                    synchronized (found) {
                        found.put(d.getAddress(), d);
                    }
                }
            } else if (BluetoothAdapter.ACTION_DISCOVERY_FINISHED.equals(a)) {
                scanning = false;
            } else if (BluetoothDevice.ACTION_BOND_STATE_CHANGED.equals(a)) {
                BluetoothDevice d = i.getParcelableExtra(BluetoothDevice.EXTRA_DEVICE);
                int st = i.getIntExtra(BluetoothDevice.EXTRA_BOND_STATE, BluetoothDevice.BOND_NONE);
                if (d == null || !d.getAddress().equals(pairingWith)) return;
                if (st == BluetoothDevice.BOND_BONDED) {
                    pairingWith = "";
                    synchronized (found) {
                        found.remove(d.getAddress());
                    }
                    // Emparejado: ahora sí, como teclado.
                    if (hid != null) {
                        state = "connecting";
                        hid.connect(d);
                    }
                } else if (st == BluetoothDevice.BOND_NONE) {
                    pairingWith = "";
                    if (host == null) state = "ready";
                    error = "El otro equipo no aceptó el emparejamiento.";
                }
            }
        }
    };

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
                BluetoothHidDevice.SUBCLASS1_COMBO, HidReports.DESCRIPTOR);
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
                byte[] r = id == ID_MOUSE ? new byte[5] : id == ID_MEDIA ? new byte[2] : reports.keyReport();
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

    public void handle(String json) {
        if (hid == null || host == null) return;
        reports.handle(json, sink);
    }

    private void clear() {
        reports.clear();
    }

    public void releaseAll() {
        if (hid != null && host != null) reports.releaseAll(sink);
        else reports.clear();
    }

    public String state() {
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
    public String devices() {
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

    public void connect(String address) {
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

    // ── Buscar y emparejar desde la tablet ────────────────────────────────

    /** Busca equipos cercanos unos 12 s (el otro equipo tiene que estar visible). */
    public void scan() {
        if (adapter == null) return;
        if (!receiverOn) {
            IntentFilter f = new IntentFilter(BluetoothDevice.ACTION_FOUND);
            f.addAction(BluetoothAdapter.ACTION_DISCOVERY_FINISHED);
            f.addAction(BluetoothDevice.ACTION_BOND_STATE_CHANGED);
            ctx.registerReceiver(receiver, f);
            receiverOn = true;
        }
        synchronized (found) {
            found.clear();
        }
        if (adapter.isDiscovering()) adapter.cancelDiscovery();
        scanning = adapter.startDiscovery();
        if (!scanning) error = "No se pudo buscar (¿falta el permiso de dispositivos cercanos o la ubicación?)";
    }

    public String found() {
        JSONArray a = new JSONArray();
        synchronized (found) {
            for (BluetoothDevice d : found.values()) {
                try {
                    a.put(new JSONObject().put("address", d.getAddress()).put("name", name(d))
                            .put("pairing", d.getAddress().equals(pairingWith)));
                } catch (JSONException ignored) {
                }
            }
        }
        try {
            return new JSONObject().put("scanning", scanning).put("devices", a).toString();
        } catch (JSONException e) {
            return "{\"scanning\":false,\"devices\":[]}";
        }
    }

    /**
     * Empieza la tablet el emparejamiento. El otro equipo suele pedir una
     * confirmación (un clic o comparar un número) y luego se conecta solo.
     */
    public void pair(String address) {
        BluetoothDevice d;
        synchronized (found) {
            d = found.get(address);
        }
        if (d == null) return;
        if (adapter.isDiscovering()) adapter.cancelDiscovery(); // buscar frena el emparejamiento
        scanning = false;
        pairingWith = address;
        error = "";
        state = "connecting";
        if (!d.createBond()) {
            pairingWith = "";
            state = host == null ? "ready" : "connected";
            error = "No se pudo empezar el emparejamiento.";
        }
    }

    public void close() {
        if (receiverOn) {
            try {
                ctx.unregisterReceiver(receiver);
            } catch (IllegalArgumentException ignored) {
            }
            receiverOn = false;
        }
        if (adapter != null && scanning) adapter.cancelDiscovery();
        releaseAll();
        if (hid != null) {
            if (host != null) hid.disconnect(host);
            hid.unregisterApp();
            adapter.closeProfileProxy(BluetoothProfile.HID_DEVICE, hid);
            hid = null;
        }
    }
}
