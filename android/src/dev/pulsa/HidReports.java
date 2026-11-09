package dev.pulsa;

import java.util.ArrayList;
import java.util.List;

import org.json.JSONException;
import org.json.JSONObject;

/**
 * Traduce los mensajes del teclado web (códigos evdev) a informes HID:
 * teclado (id 1, 8 bytes), ratón con rueda (id 2, 5 bytes) y multimedia
 * (id 3, 2 bytes). Lo usan el Bluetooth directo ({@link BtHid}) y el puente
 * USB ({@link Bridge}), que sólo cambian por dónde sale cada informe.
 */
final class HidReports {
    static final byte ID_KEYS = 1, ID_MOUSE = 2, ID_MEDIA = 3;

    /** Por dónde sale cada informe. */
    interface Sink {
        void send(byte id, byte[] report);
    }

    /** Descriptor HID: teclado de arranque + ratón con rueda y desplazamiento lateral + multimedia. */
    static final byte[] DESCRIPTOR = bytes(
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

    private int modifiers;
    private final List<Integer> keys = new ArrayList<>();
    private int buttons;
    private double restX, restY;
    private int restWheel, restPan;
    private int media;

    /** Mensaje de la página ({@code {"t":"k","c":30,"d":true}}…) → informes. */
    synchronized void handle(String json, Sink out) {
        try {
            JSONObject m = new JSONObject(json);
            switch (m.optString("t")) {
                case "k": key(m.optInt("c"), m.optBoolean("d"), out); break;
                case "b": button(m.optInt("b"), m.optBoolean("d"), out); break;
                case "m": move(m.optDouble("x", 0), m.optDouble("y", 0), out); break;
                case "s": scroll(m.optInt("x"), m.optInt("y"), out); break;
                default: break;
            }
        } catch (JSONException e) {
            android.util.Log.w("Pulsa", "mensaje no válido: " + json);
        }
    }

    private void key(int code, boolean down, Sink out) {
        int bit = modifierBit(code);
        int mediaUse = mediaUsage(code);
        if (bit >= 0) {
            modifiers = down ? modifiers | (1 << bit) : modifiers & ~(1 << bit);
        } else if (mediaUse != 0) {
            media = down ? mediaUse : 0;
            out.send(ID_MEDIA, new byte[]{(byte) media, (byte) (media >> 8)});
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
        out.send(ID_KEYS, keyReport());
    }

    byte[] keyReport() {
        byte[] r = new byte[8];
        r[0] = (byte) modifiers;
        for (int i = 0; i < keys.size(); i++) r[2 + i] = (byte) (int) keys.get(i);
        return r;
    }

    private void button(int b, boolean down, Sink out) {
        int bit = b == 2 ? 2 : b == 1 ? 4 : 1;
        buttons = down ? buttons | bit : buttons & ~bit;
        mouse(0, 0, 0, 0, out);
    }

    private void move(double x, double y, Sink out) {
        restX += x;
        restY += y;
        int ix = (int) restX, iy = (int) restY;
        restX -= ix;
        restY -= iy;
        // Un informe lleva como mucho ±127 por eje.
        while (ix != 0 || iy != 0) {
            int sx = Math.max(-127, Math.min(127, ix)), sy = Math.max(-127, Math.min(127, iy));
            mouse(sx, sy, 0, 0, out);
            ix -= sx;
            iy -= sy;
        }
    }

    /** En unidades de 120 por muesca, como el servidor del PC. */
    private void scroll(int x, int y, Sink out) {
        restWheel += y;
        restPan += x;
        int w = restWheel / 120, p = restPan / 120;
        restWheel -= w * 120;
        restPan -= p * 120;
        if (w != 0 || p != 0) mouse(0, 0, w, p, out);
    }

    private void mouse(int x, int y, int wheel, int pan, Sink out) {
        out.send(ID_MOUSE, new byte[]{(byte) buttons, (byte) x, (byte) y,
                (byte) Math.max(-127, Math.min(127, wheel)), (byte) Math.max(-127, Math.min(127, pan))});
    }

    /** Suelta todo (la página se fue o el equipo se desconectó). */
    synchronized void clear() {
        modifiers = 0;
        keys.clear();
        buttons = 0;
        media = 0;
    }

    /** Suelta todo y lo comunica (antes de irse o al perder la página). */
    synchronized void releaseAll(Sink out) {
        clear();
        out.send(ID_KEYS, keyReport());
        mouse(0, 0, 0, 0, out);
        out.send(ID_MEDIA, new byte[2]);
    }

}
