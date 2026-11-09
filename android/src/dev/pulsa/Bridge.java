package dev.pulsa;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.bluetooth.le.BluetoothLeScanner;
import android.bluetooth.le.ScanCallback;
import android.bluetooth.le.ScanFilter;
import android.bluetooth.le.ScanResult;
import android.bluetooth.le.ScanSettings;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelUuid;
import android.util.Log;

import java.security.SecureRandom;
import java.util.Collections;
import java.util.UUID;
import java.util.concurrent.ConcurrentLinkedDeque;

import org.json.JSONException;
import org.json.JSONObject;

/**
 * Puente USB: una placa ESP32-S3 enchufada al otro equipo hace de teclado y
 * ratón USB de verdad, y la tablet le manda los informes HID por Bluetooth
 * de bajo consumo (BLE). El otro equipo no tiene que emparejar nada: ve un
 * teclado USB. El programa de la placa está en {@code firmware/pulsa-puente}.
 *
 * Protocolo (característica RX, cada escritura es un paquete):
 * <ul>
 *   <li>{@code [0xA0, clave(16)]}: quién soy. La placa se queda con la primera
 *       clave que recibe y rechaza las demás (se libera con BOOT 3 s).</li>
 *   <li>{@code [id, informe…]}: informe HID (ids de {@link HidReports}).</li>
 * </ul>
 * La placa avisa por la característica LED (notificaciones):
 * {@code [0xA1, 1|0]} clave aceptada o no; {@code [0xA2, leds]} Bloq Num (bit 0) y Mayús (bit 1).
 */
@TargetApi(Build.VERSION_CODES.LOLLIPOP)
@SuppressLint("MissingPermission") // los permisos se piden antes de crear esto
final class Bridge implements Link {
    private static final String TAG = "PulsaPuente";
    static final UUID SERVICE = UUID.fromString("c9f80001-5e8b-4f3a-9c6e-70756c736121");
    static final UUID RX = UUID.fromString("c9f80002-5e8b-4f3a-9c6e-70756c736121");
    static final UUID LED = UUID.fromString("c9f80003-5e8b-4f3a-9c6e-70756c736121");
    private static final UUID CCCD = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");
    private static final byte AUTH = (byte) 0xA0, AUTH_OK = (byte) 0xA1, LEDS = (byte) 0xA2;
    private static final int MAX_QUEUE = 256;

    private final Context ctx;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final BluetoothAdapter adapter;
    private final HidReports reports = new HidReports();
    private final ConcurrentLinkedDeque<byte[]> queue = new ConcurrentLinkedDeque<>();
    private BluetoothGatt gatt;
    private BluetoothGattCharacteristic rx;
    private volatile boolean busy, authed, closed;
    private volatile String state = "starting";
    private volatile String error = "";
    private volatile boolean caps, num;
    private int authTries;

    private final HidReports.Sink sink = (id, r) -> {
        if (!authed) return;
        byte[] p = new byte[r.length + 1];
        p[0] = id;
        System.arraycopy(r, 0, p, 1, r.length);
        // Si la conexión va lenta, sobran antes movimientos de ratón que teclas.
        if (queue.size() >= MAX_QUEUE) {
            for (byte[] q : queue) {
                if (q[0] == HidReports.ID_MOUSE) {
                    queue.remove(q);
                    break;
                }
            }
        }
        queue.add(p);
        pump();
    };

    Bridge(Context ctx) {
        this.ctx = ctx.getApplicationContext();
        BluetoothManager bm = (BluetoothManager) ctx.getSystemService(Context.BLUETOOTH_SERVICE);
        adapter = bm != null ? bm.getAdapter() : null;
        if (adapter == null || !adapter.isEnabled()) {
            fail("Activa el Bluetooth de la tablet.");
            return;
        }
        search();
    }

    private void fail(String why) {
        Log.w(TAG, why);
        error = why;
        state = "error";
    }

    private SharedPreferences prefs() {
        return ctx.getSharedPreferences("pulsa", Context.MODE_PRIVATE);
    }

    /** Clave fija de esta tablet ante la placa. */
    private byte[] key() {
        String hex = prefs().getString("bridge_key", "");
        if (hex.length() != 32) {
            byte[] k = new byte[16];
            new SecureRandom().nextBytes(k);
            StringBuilder sb = new StringBuilder();
            for (byte b : k) sb.append(String.format("%02x", b));
            hex = sb.toString();
            prefs().edit().putString("bridge_key", hex).apply();
        }
        byte[] k = new byte[16];
        for (int i = 0; i < 16; i++) k[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
        return k;
    }

    // ── Buscar y conectar ──────────────────────────────────────────────────

    private final ScanCallback scanCb = new ScanCallback() {
        @Override
        public void onScanResult(int type, ScanResult result) {
            stopScan();
            connect(result.getDevice());
        }

        @Override
        public void onScanFailed(int code) {
            fail("No se pudo buscar el puente (error " + code + ").");
        }
    };

    private void search() {
        if (closed) return;
        state = "ready";
        BluetoothLeScanner sc = adapter.getBluetoothLeScanner();
        if (sc == null) {
            fail("Activa el Bluetooth de la tablet.");
            return;
        }
        ScanFilter f = new ScanFilter.Builder().setServiceUuid(new ParcelUuid(SERVICE)).build();
        ScanSettings st = new ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build();
        sc.startScan(Collections.singletonList(f), st, scanCb);
    }

    private void stopScan() {
        BluetoothLeScanner sc = adapter.getBluetoothLeScanner();
        if (sc != null) sc.stopScan(scanCb);
    }

    private void connect(BluetoothDevice d) {
        if (closed) return;
        state = "connecting";
        gatt = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M
                ? d.connectGatt(ctx, false, gattCb, BluetoothDevice.TRANSPORT_LE)
                : d.connectGatt(ctx, false, gattCb);
    }

    private final BluetoothGattCallback gattCb = new BluetoothGattCallback() {
        @Override
        public void onConnectionStateChange(BluetoothGatt g, int status, int newState) {
            if (newState == BluetoothProfile.STATE_CONNECTED) {
                g.requestConnectionPriority(BluetoothGatt.CONNECTION_PRIORITY_HIGH); // menos retraso
                g.discoverServices();
            } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                g.close();
                if (g == gatt) gatt = null;
                rx = null;
                authed = false;
                busy = false;
                queue.clear();
                reports.clear();
                if (!closed && !"error".equals(state)) {
                    state = "ready";
                    main.postDelayed(Bridge.this::search, 800);
                }
            }
        }

        @Override
        public void onServicesDiscovered(BluetoothGatt g, int status) {
            BluetoothGattService s = g.getService(SERVICE);
            if (s == null) {
                fail("Eso no es un puente de Pulsa.");
                g.disconnect();
                return;
            }
            rx = s.getCharacteristic(RX);
            BluetoothGattCharacteristic led = s.getCharacteristic(LED);
            if (led != null) {
                g.setCharacteristicNotification(led, true);
                BluetoothGattDescriptor d = led.getDescriptor(CCCD);
                if (d != null) {
                    d.setValue(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE);
                    busy = true; // la clave sale cuando se haya escrito esto
                    g.writeDescriptor(d);
                    return;
                }
            }
            sendAuth();
        }

        @Override
        public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {
            busy = false;
            sendAuth();
        }

        @Override
        public void onCharacteristicWrite(BluetoothGatt g, BluetoothGattCharacteristic c, int status) {
            busy = false;
            if (!authed && status != BluetoothGatt.GATT_SUCCESS) {
                // Mientras Android cifra el enlace (primera vez) puede fallar: reintentar.
                if (++authTries < 10) main.postDelayed(Bridge.this::sendAuth, 700);
                else fail("El puente no responde.");
                return;
            }
            pump();
        }

        @Override
        @SuppressWarnings("deprecation")
        public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic c) {
            byte[] v = c.getValue();
            if (v == null || v.length < 2) return;
            if (v[0] == AUTH_OK) {
                if (v[1] == 1) {
                    authed = true;
                    error = "";
                    state = "connected";
                    prefs().edit().putString("bridge_last", g.getDevice().getAddress()).apply();
                } else {
                    fail("Este puente ya está emparejado con otra tablet. Mantén pulsado su botón BOOT 3 segundos para liberarlo.");
                    g.disconnect();
                }
            } else if (v[0] == LEDS) {
                num = (v[1] & 1) != 0;
                caps = (v[1] & 2) != 0;
            }
        }
    };

    private void sendAuth() {
        BluetoothGatt g = gatt;
        if (g == null || rx == null) return;
        byte[] k = key();
        byte[] p = new byte[17];
        p[0] = AUTH;
        System.arraycopy(k, 0, p, 1, 16);
        write(g, p, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT);
    }

    /** Un paquete cada vez: Android no admite otra escritura hasta que acaba la anterior. */
    private synchronized void pump() {
        BluetoothGatt g = gatt;
        if (busy || g == null || rx == null || !authed) return;
        byte[] p = queue.poll();
        if (p != null) write(g, p, BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE);
    }

    @SuppressWarnings("deprecation")
    private void write(BluetoothGatt g, byte[] p, int type) {
        rx.setWriteType(type);
        rx.setValue(p);
        busy = g.writeCharacteristic(rx);
        if (!busy && type == BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE) {
            queue.addFirst(p); // ocupado: se reintenta en la próxima
            main.postDelayed(this::pump, 5);
        }
    }

    // ── Link ───────────────────────────────────────────────────────────────

    @Override
    public void handle(String json) {
        if (authed) reports.handle(json, sink);
    }

    @Override
    public void releaseAll() {
        if (authed) reports.releaseAll(sink);
        else reports.clear();
    }

    @Override
    public String state() {
        try {
            JSONObject o = new JSONObject().put("state", state).put("error", error)
                    .put("caps", caps).put("num", num).put("mode", "bridge");
            if (authed) o.put("device", "Puente USB");
            return o.toString();
        } catch (JSONException e) {
            return "{\"state\":\"error\"}";
        }
    }

    @Override
    public void close() {
        closed = true;
        releaseAll();
        main.removeCallbacksAndMessages(null);
        stopScan();
        BluetoothGatt g = gatt;
        if (g != null) {
            g.disconnect();
            g.close();
        }
        gatt = null;
    }
}
