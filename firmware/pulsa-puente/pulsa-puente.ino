// Pulsa Puente: una ESP32-S3 enchufada por USB a cualquier equipo hace de
// teclado y ratón USB, y recibe lo que se pulsa en la app Pulsa de la tablet
// por Bluetooth de bajo consumo (BLE). El equipo no tiene que emparejar ni
// instalar nada: ve un teclado USB normal.
//
// Placa: ESP32-S3 (DevKitC-1, SuperMini…). En Arduino IDE: placa
// "ESP32S3 Dev Module" y Herramientas → USB Mode → "USB-OTG (TinyUSB)".
// Ver firmware/README.md.
//
// Protocolo (característica RX, cada escritura es un paquete):
//   [0xA0, clave(16)]  quién soy. La placa se queda con la primera clave que
//                      recibe; después sólo acepta esa tablet. Para liberarla,
//                      mantener pulsado BOOT 3 segundos.
//   [id, informe…]     informe HID: 1 teclado (8 bytes), 2 ratón (5), 3 multimedia (2).
// Avisos a la tablet (característica LED, notificaciones):
//   [0xA1, 1|0]        clave aceptada o no
//   [0xA2, leds]       Bloq Num (bit 0) y Bloq Mayús (bit 1) que pone el equipo

#include <Arduino.h>
#include <Preferences.h>
#include "USB.h"
#include "USBHID.h"
#include <BLEDevice.h>
#include <BLEServer.h>
#include <BLESecurity.h>
#include <BLE2902.h>

#if !SOC_USB_OTG_SUPPORTED || ARDUINO_USB_MODE
#error "Hace falta una ESP32-S3 con USB Mode = USB-OTG (TinyUSB)"
#endif

#define SERVICE_UUID "c9f80001-5e8b-4f3a-9c6e-70756c736121"
#define RX_UUID      "c9f80002-5e8b-4f3a-9c6e-70756c736121"
#define LED_UUID     "c9f80003-5e8b-4f3a-9c6e-70756c736121"

static const uint8_t AUTH = 0xA0, AUTH_OK = 0xA1, LEDS = 0xA2;
static const int BOOT_PIN = 0;

// El mismo descriptor que la app usa en modo Bluetooth (HidReports.java):
// teclado (id 1), ratón con rueda y desplazamiento lateral (id 2), multimedia (id 3).
static const uint8_t DESCRIPTOR[] = {
  // Teclado
  0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,
  0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
  0x95, 0x01, 0x75, 0x08, 0x81, 0x01,
  0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02,
  0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
  0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00,
  0xC0,
  // Ratón
  0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00,
  0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02,
  0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
  0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06,
  0x05, 0x0C, 0x0A, 0x38, 0x02, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
  0xC0, 0xC0,
  // Multimedia
  0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x03,
  0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, 0x75, 0x10, 0x95, 0x01, 0x81, 0x00,
  0xC0,
};

static size_t reportLen(uint8_t id) {
  switch (id) {
    case 1: return 8;
    case 2: return 5;
    case 3: return 2;
    default: return 0;
  }
}

// ── USB ───────────────────────────────────────────────────────────────────

USBHID HID;
static volatile uint8_t hostLeds = 0;
static volatile bool ledsChanged = false;

class PulsaHid : public USBHIDDevice {
 public:
  PulsaHid() {
    static bool added = false;
    if (!added) {
      added = true;
      HID.addDevice(this, sizeof(DESCRIPTOR));
    }
  }
  uint16_t _onGetDescriptor(uint8_t *buffer) override {
    memcpy(buffer, DESCRIPTOR, sizeof(DESCRIPTOR));
    return sizeof(DESCRIPTOR);
  }
  // El equipo enciende Bloq Num / Bloq Mayús: informe de salida del teclado.
  void _onOutput(uint8_t report_id, const uint8_t *buffer, uint16_t len) override {
    if (len == 0) return;
    hostLeds = buffer[len - 1];
    ledsChanged = true;
  }
  bool send(uint8_t id, const uint8_t *data, size_t len) {
    return HID.SendReport(id, data, len);
  }
};

PulsaHid hid;

// Lo que llega por BLE se mete aquí y lo manda loop(): enviar por USB puede
// esperar al equipo, y eso no debe frenar la pila Bluetooth.
struct Packet {
  uint8_t id;
  uint8_t len;
  uint8_t data[8];
};
static QueueHandle_t queue;

static void releaseAll() {
  static const uint8_t zero[8] = {0};
  for (uint8_t id = 1; id <= 3; id++) {
    Packet p = {id, (uint8_t)reportLen(id), {0}};
    memcpy(p.data, zero, p.len);
    xQueueSend(queue, &p, 0);
  }
}

// ── BLE ───────────────────────────────────────────────────────────────────

Preferences prefs;
static uint8_t savedKey[16];
static bool hasKey = false;
static volatile bool connected = false, authed = false, rejected = false;
static BLECharacteristic *ledChar = nullptr;
static BLEServer *server = nullptr;

static void notify(uint8_t kind, uint8_t value) {
  if (!ledChar || !connected) return;
  uint8_t v[2] = {kind, value};
  ledChar->setValue(v, 2);
  ledChar->notify();
}

class ServerCallbacks : public BLEServerCallbacks {
  void onConnect(BLEServer *s) override {
    connected = true;
    authed = false;
    rejected = false;
  }
  void onDisconnect(BLEServer *s) override {
    connected = false;
    authed = false;
    releaseAll();  // que no se quede ninguna tecla pegada
    BLEDevice::startAdvertising();
  }
};

class RxCallbacks : public BLECharacteristicCallbacks {
  void onWrite(BLECharacteristic *c) override {
    auto value = c->getValue();
    const uint8_t *d = (const uint8_t *)value.c_str();
    size_t n = value.length();
    if (n == 0) return;
    if (d[0] == AUTH && n == 17) {
      if (!hasKey) {
        memcpy(savedKey, d + 1, 16);
        prefs.putBytes("key", savedKey, 16);
        hasKey = true;
      }
      authed = memcmp(savedKey, d + 1, 16) == 0;
      rejected = !authed;
      notify(AUTH_OK, authed ? 1 : 0);
      if (authed) notify(LEDS, hostLeds);
      return;
    }
    size_t len = reportLen(d[0]);
    if (!authed || len == 0 || n != len + 1) return;
    Packet p = {d[0], (uint8_t)len, {0}};
    memcpy(p.data, d + 1, len);
    xQueueSend(queue, &p, pdMS_TO_TICKS(5));
  }
};

// ── LED de la placa (si tiene uno RGB): azul esperando, verde conectada, rojo rechazada ──

static void showStatus() {
#ifdef RGB_BUILTIN
  static uint32_t last = 0;
  if (millis() - last < 100) return;
  last = millis();
  uint8_t r = 0, g = 0, b = 0;
  if (rejected) r = 30;
  else if (authed) g = 20;
  else if ((millis() / 500) % 2) b = 30;  // parpadeo: esperando a la tablet
  neopixelWrite(RGB_BUILTIN, r, g, b);
#endif
}

void setup() {
  pinMode(BOOT_PIN, INPUT_PULLUP);
  queue = xQueueCreate(64, sizeof(Packet));

  prefs.begin("pulsa", false);
  hasKey = prefs.getBytes("key", savedKey, 16) == 16;

  USB.productName("Pulsa Puente");
  USB.manufacturerName("Pulsa");
  HID.begin();
  USB.begin();

  BLEDevice::init("Pulsa Puente");
  // Enlace cifrado: lo que escribes no viaja en claro por el aire.
  BLEDevice::setEncryptionLevel(ESP_BLE_SEC_ENCRYPT_NO_MITM);
  BLESecurity *security = new BLESecurity();
  security->setAuthenticationMode(ESP_LE_AUTH_REQ_SC_BOND);
  security->setCapability(ESP_IO_CAP_NONE);
  security->setInitEncryptionKeys(ESP_BLE_ENC_KEY_MASK | ESP_BLE_ID_KEY_MASK);

  server = BLEDevice::createServer();
  server->setCallbacks(new ServerCallbacks());
  BLEService *service = server->createService(SERVICE_UUID);
  BLECharacteristic *rx = service->createCharacteristic(
      RX_UUID, BLECharacteristic::PROPERTY_WRITE | BLECharacteristic::PROPERTY_WRITE_NR);
  rx->setAccessPermissions(ESP_GATT_PERM_WRITE_ENCRYPTED);
  rx->setCallbacks(new RxCallbacks());
  ledChar = service->createCharacteristic(
      LED_UUID, BLECharacteristic::PROPERTY_READ | BLECharacteristic::PROPERTY_NOTIFY);
  ledChar->addDescriptor(new BLE2902());
  service->start();

  BLEAdvertising *adv = BLEDevice::getAdvertising();
  adv->addServiceUUID(SERVICE_UUID);
  adv->setScanResponse(true);
  BLEDevice::startAdvertising();
}

void loop() {
  Packet p;
  while (xQueueReceive(queue, &p, pdMS_TO_TICKS(10)) == pdTRUE) {
    hid.send(p.id, p.data, p.len);
  }
  if (ledsChanged) {
    ledsChanged = false;
    if (authed) notify(LEDS, hostLeds);
  }

  // BOOT 3 s: olvidar la tablet y volver a empezar.
  static uint32_t pressedAt = 0;
  if (digitalRead(BOOT_PIN) == LOW) {
    if (!pressedAt) pressedAt = millis();
    else if (millis() - pressedAt > 3000) {
      prefs.remove("key");
#ifdef RGB_BUILTIN
      neopixelWrite(RGB_BUILTIN, 30, 30, 30);
#endif
      delay(300);
      ESP.restart();
    }
  } else {
    pressedAt = 0;
  }
  showStatus();
}
