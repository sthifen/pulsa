# Puente USB de Pulsa

Una placa diminuta enchufada al USB del equipo que hace de **teclado y ratón USB de verdad**. La tablet le manda
lo que pulsas por Bluetooth de bajo consumo:

```
Tablet (app Pulsa) ──Bluetooth──► ESP32-S3 ──cable USB──► equipo
```

- El equipo **no tiene que emparejar ni instalar nada**: ve un teclado USB normal. Sirve para alguien que se quedó
  sin teclado ni ratón y no puede ni aceptar un emparejamiento Bluetooth.
- Funciona en Windows, macOS, Linux, Raspberry Pi, Android, consolas, smart TV… y en la pantalla de inicio de sesión.
- Lo que escribes viaja **cifrado**, y la placa sólo obedece a la primera tablet que se conecta.

## Qué placa comprar

Tiene que ser una **ESP32-S3**: es la que tiene a la vez Bluetooth y USB nativo. (Una ESP32 normal o una ESP32-C3
no hace de teclado USB, y una ESP32-S2 no tiene Bluetooth.)

| Placa | Precio aprox. | Notas |
|---|---|---|
| **ESP32-S3 SuperMini** | 3–6 USD | Diminuta, USB-C. La más práctica: se queda enchufada como un pendrive |
| ESP32-S3-DevKitC-1 (oficial de Espressif) | 10–15 USD | Tiene dos USB-C: usa el marcado **USB** (no el de **UART/COM**). Trae LED de colores |

Se consigue en AliExpress, Amazon o tiendas de electrónica buscando «ESP32-S3 SuperMini». Hace falta un cable
USB-C de datos (no sólo de carga) y, si el equipo sólo tiene USB-A, un adaptador.

## Grabar el programa (una vez, sin instalar nada)

1. Descarga `pulsa-puente-esp32s3.bin` de la [última versión](https://github.com/sthifen/pulsa/releases/latest).
2. Abre **Chrome o Edge** en un ordenador y entra en <https://espressif.github.io/esptool-js/> (la herramienta
   oficial de Espressif; funciona en el navegador).
3. Conecta la placa. Si no aparece, desenchúfala, **mantén pulsado BOOT**, enchúfala y suelta.
4. Pulsa **Connect** y elige el puerto de la placa.
5. En *Flash Address* deja **0x0**, elige el archivo `.bin` y pulsa **Program**.
6. Al terminar, desenchufa y vuelve a enchufar la placa.

<details>
<summary>Con Arduino IDE (si prefieres compilarlo tú)</summary>

1. Añade el soporte de ESP32 (Gestor de placas → «esp32» de Espressif, versión 3.0.x).
2. Abre `firmware/pulsa-puente/pulsa-puente.ino`.
3. Placa: **ESP32S3 Dev Module**; Herramientas → **USB Mode → USB-OTG (TinyUSB)**.
4. Sube el programa (si no conecta, mantén BOOT al enchufar).
</details>

## Usarla

1. Enchufa la placa al equipo que quieres manejar.
2. En la tablet abre Pulsa → **Usar con el puente USB**. La primera vez Android puede preguntar si emparejar con
   «Pulsa Puente»: acepta.
3. Escribe. La próxima vez se conecta sola.

LED de la placa (si tiene uno de colores): **azul parpadeando** esperando a la tablet, **verde** conectada,
**rojo** pertenece a otra tablet.

**Para que la use otra tablet:** mantén pulsado **BOOT** 3 segundos; la placa olvida la tablet anterior y se reinicia.

## Limitaciones conocidas

- Todavía no se ha probado con una placa real: si algo falla, abre un *issue*.
- En la BIOS/UEFI de algunos equipos puede no funcionar: esos menús esperan un teclado en «modo arranque», que este
  primer programa aún no implementa. En el sistema operativo y su pantalla de inicio de sesión, sí.
