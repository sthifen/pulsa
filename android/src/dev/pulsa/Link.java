package dev.pulsa;

/**
 * Por dónde salen las teclas cuando no hay PC con Pulsa: el Bluetooth
 * directo ({@link BtHid}) o el puente USB ({@link Bridge}). La página habla
 * con el que esté activo a través de MainActivity.
 */
interface Link {
    void handle(String json);

    void releaseAll();

    /** {@code {"state":"ready|connecting|connected|error","error":…,"device":…,"caps":…,"num":…}} */
    String state();

    void close();

    // Sólo el Bluetooth directo: equipos emparejados, buscar y emparejar.
    default String devices() {
        return "[]";
    }

    default void connect(String address) {
    }

    default void scan() {
    }

    default String found() {
        return "{\"scanning\":false,\"devices\":[]}";
    }

    default void pair(String address) {
    }
}
