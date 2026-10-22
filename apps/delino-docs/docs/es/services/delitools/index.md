# DeliTools

DeliTools proporciona herramientas de IA para desarrolladores mediante una API de Connect. La versión 1 tiene dos métodos síncronos: la **traducción** de una matriz de textos y la **extracción de datos** de un texto a JSON que sigue tu esquema. Cada llamada devuelve el resultado final o un error en la misma respuesta. No hay trabajos que consultar ni resultados que recuperar más tarde. Todos los métodos están en `delitools.v1.DeliToolsService`.

Llama a DeliTools desde código del servidor o desde la línea de comandos, con cualquier cliente de Connect o mediante JSON simple sobre HTTP. No pongas una clave de API en el código del frontend. Establece `DELITOOLS_URL` en la URL base de tu servidor de API de DeliTools.

## Inicio rápido

1. **Elige una credencial.** Utiliza tu token de inicio de sesión de Delino o crea una clave de API con `CreateKey`. Consulta la sección Autenticación y claves de API.
2. **Busca el ID de tu tenant.** El uso se cobra al tenant de Delino que indiques en `tenantId`.
3. **Llama a un método** con `Authorization: Bearer <credential>`.

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

En JSON, los nombres de los campos utilizan lowerCamelCase y los valores de enumeración utilizan sus nombres completos.

## Traducir texto

`TranslateTextArray` traduce cada cadena de una matriz.

| Campo            | Obligatorio | Significado                                                                                                |
| ---------------- | ----------- | ---------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Sí          | El tenant que recibe el cargo por el uso. Utiliza su UUID versión 7 canónico: en minúsculas y con guiones. |
| `outputLanguage` | Sí          | Etiqueta de idioma BCP 47 de la traducción, por ejemplo `ko`, `pt-BR` o `zh-Hans`.                         |
| `inputLanguage`  | No          | Etiqueta de idioma BCP 47 del origen. Omítela para detectar un único idioma para toda la matriz.           |
| `model`          | Sí          | Un modelo de la lista de permitidos del operador.                                                          |
| `texts`          | Sí          | Las cadenas que se van a traducir.                                                                         |
| `idempotencyKey` | No          | Consulta Idempotencia.                                                                                     |

La respuesta contiene `requestId` y `texts`, las traducciones en el orden original y con la longitud original.

Reglas:

- Los valores de idioma son etiquetas BCP 47. `outputLanguage` es obligatorio. Una etiqueta que no sea BCP 47 válida, que no sea ASCII o que supere los 64 bytes devuelve un error de entrada no válida.
- `texts` no debe estar vacío y ninguna cadena puede estar vacía ni contener solo espacios en blanco.
- Si estableces `inputLanguage` y es igual a `outputLanguage`, la solicitud no es válida.
- Si omites `inputLanguage`, DeliTools detecta un único idioma de entrada para toda la matriz. Una matriz con varios idiomas se traduce igualmente. Si el idioma detectado es igual a `outputLanguage`, el modelo sigue procesando cada elemento. DeliTools no devuelve la matriz original sin procesar.
- Solo se aceptan los modelos de la lista de permitidos del operador. Cualquier otro modelo devuelve un error de modelo no válido. Un modelo que existe en el catálogo de Vercel AI Gateway no está permitido automáticamente.
- La llamada solo se completa correctamente si todos los elementos se completan correctamente. Un elemento fallido hace fallar toda la solicitud. DeliTools nunca devuelve traducciones parciales y nunca sustituye el texto de origen por un elemento fallido.

## Extraer datos

`ExtractDataFromText` lee un texto y devuelve JSON que sigue tu esquema JSON. El servicio utiliza un modelo fijo, `gpt-6-luna`. Tú no eliges el modelo.

| Campo            | Obligatorio | Significado                                                                                                |
| ---------------- | ----------- | ---------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Sí          | El tenant que recibe el cargo por el uso. Utiliza su UUID versión 7 canónico: en minúsculas y con guiones. |
| `text`           | Sí          | El texto de origen. No puede estar vacío ni contener solo espacios en blanco.                              |
| `jsonSchema`     | Sí          | Un esquema JSON, como cadena, que utiliza el subconjunto compatible.                                       |
| `idempotencyKey` | No          | Consulta Idempotencia.                                                                                     |

La respuesta contiene `requestId` y `jsonData`, una cadena JSON que DeliTools validó con tu esquema.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

Cómo funciona la extracción:

- Se indica al modelo que utilice solo la información que el texto expresa. Si al texto le falta información que el esquema exige, la llamada falla con un error de extracción imposible. El servicio no inventa un valor para satisfacer el esquema.
- DeliTools comprueba en el servidor la salida del modelo con tu esquema. Una salida mal formada o que no se ajusta al esquema es un fallo con un error de salida no válida. Nunca se devuelve como resultado y DeliTools no la reintenta ni la repara.
- Una comprobación del esquema y un prompt no garantizan que todos los valores sean semánticamente correctos. Revisa los datos extraídos antes de basarte en ellos. DeliTools no devuelve evidencias de cada valor.
- DeliTools comprueba el esquema antes de llamar a cualquier modelo de IA. Un esquema fuera del subconjunto compatible devuelve un error de esquema no válido, sin llamada a la IA y sin cargo.

### Subconjunto de esquema

DeliTools acepta solo la parte de JSON Schema que admite la salida estructurada del modelo de extracción.

Compatible:

- Un objeto en la raíz.
- Los tipos `object`, `array`, `string`, `number`, `integer`, `boolean` y `null`, anidados hasta una profundidad de 8 niveles. Cada nodo del esquema tiene exactamente un `type`, escrito como cadena. Se rechaza un arreglo de tipos, como `["string", "null"]`.
- `properties`, con todas las propiedades indicadas en `required`. El nombre de una propiedad tiene de 1 a 256 caracteres.
- `additionalProperties` establecido en `false` en todos los objetos.
- `items` para las matrices.
- `description` en cualquier esquema.
- `enum` con valores escalares. Una enumeración no puede estar vacía, sus valores deben ser únicos y coincidir con el `type` del nodo, y una enumeración `integer` solo admite números enteros.

No compatible y rechazado antes de la ejecución:

- Cualquier palabra clave que no esté en la lista anterior, por ejemplo `pattern`, `minimum`, `format` o `oneOf`.
- Propiedades opcionales. Todas las propiedades de un objeto deben estar en `required`. `required` debe enumerar cada propiedad exactamente una vez y no debe nombrar una propiedad que `properties` no defina.
- `additionalProperties` establecido en `true` o ausente.
- Referencias: `$ref` y esquemas recursivos.
- Esquemas que superen los límites del proveedor: 4.998 propiedades en total, 1.000 valores de enumeración o 119.980 caracteres entre los nombres de propiedades y los valores de enumeración en conjunto. El proveedor permite en total 5.000 propiedades y 120.000 caracteres, y DeliTools reserva el resto para su sobre de respuesta. Una enumeración con más de 250 valores puede contener como máximo 15.000 caracteres de valores de cadena.

Un esquema compatible:

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

Un esquema no compatible, porque `note` es opcional y `pattern` no es compatible:

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## Comportamiento de las solicitudes

- Ambos métodos son síncronos. Mantén la conexión abierta hasta recibir un resultado o un error.
- Cada éxito y cada error contiene un ID de solicitud, un UUID versión 7. Inclúyelo cuando solicites asistencia. Las respuestas correctas contienen solo el resultado y el ID de solicitud. No contienen el modelo, el recuento de tokens ni el cargo. Consulta Facturación para revisar el uso.
- DeliTools reintenta los errores transitorios del proveedor de IA, hasta un límite que establece el operador. No reintenta una salida mal formada y nunca cambia a otro modelo por su cuenta.
- Si cancelas la llamada o te desconectas, DeliTools intenta cancelar la llamada a la IA y detiene los reintentos posteriores. El uso que ya se haya producido se sigue midiendo y facturando.
- El operador establece los límites del tamaño de entrada, del número de elementos de traducción, del tiempo de espera de ejecución, de la frecuencia de solicitudes por tenant y de las solicitudes simultáneas por tenant. Son ajustes de la implementación, no límites fijos del producto. Una solicitud que supera un límite devuelve un error de límite. La excepción es el tiempo de espera de ejecución: una llamada que lo supera devuelve un error de tiempo de espera (`ERROR_REASON_TIMEOUT`).

## Autenticación y claves de API

Envía `Authorization: Bearer <credential>` con cada solicitud. Dos tipos de credencial funcionan con ambas herramientas:

- **Un token de inicio de sesión de Delino** (un token de acceso de usuario de Logto) actúa como el usuario que ha iniciado sesión.
- **Una clave de API de DeliTools** actúa según lo que describe su tipo de clave.

Cada solicitud también se comprueba con tu tenant en Delcore. Para `TranslateTextArray` y `ExtractDataFromText`, el tenant debe estar activo y tener permitido ejecutar trabajo de pago, y la facturación debe permitirlo. Si DeliTools no puede completar estas comprobaciones, rechaza las nuevas ejecuciones de pago. La gestión de claves (`CreateKey`, `ListKeys` y `RevokeKey`) no es trabajo de pago: solo requiere tu pertenencia actual al tenant o tu rol de Owner o Admin, por lo que un bloqueo de facturación no te impide enumerar ni revocar claves. Si DeliTools no puede completar esa comprobación, rechaza la llamada.

### Tipos de clave y capacidades

- Una **clave personal** pertenece a un usuario. Sus permisos son la intersección de sus capacidades y los permisos actuales de ese usuario. Cuando el usuario pierde el acceso al tenant, la clave deja de funcionar.
- Una **clave de servicio** pertenece al tenant. No depende de la pertenencia del usuario que la creó. Solo un Owner o Admin del tenant puede crear y administrar claves de servicio.
- Cada clave tiene una o ambas capacidades: `CAPABILITY_TRANSLATOR` y `CAPABILITY_EXTRACTOR`. Una clave solo puede llamar a las herramientas de sus capacidades.
- Los usuarios administran sus propias claves personales. La administración de claves requiere un token de inicio de sesión de Delino. Una clave de API no puede crear ni revocar claves.

### Administrar claves

- `CreateKey`: envía `tenantId`, `kind` (`KEY_KIND_PERSONAL` o `KEY_KIND_SERVICE`), `capabilities` y un `expiresAt` opcional.
- `ListKeys`: enumera las claves de un tenant que puedes gestionar, filtradas por `kind`, con `afterId` y `pageSize` para la paginación. `pageSize` es 50 por defecto si lo omites o envías 0. El máximo es 100, y un valor mayor devuelve un error de entrada no válida. Nunca puedes enumerar las claves personales de otro usuario. Con `KEY_KIND_PERSONAL` obtienes tus propias claves personales. Con `KEY_KIND_SERVICE` obtienes las claves de servicio del tenant, pero solo si eres Owner o Admin. Sin `kind` obtienes tus propias claves personales y, si eres Owner o Admin, también las claves de servicio del tenant.
- `RevokeKey`: envía `tenantId` y `keyId`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **Secreto de un solo uso.** La respuesta de `CreateKey` contiene el `secret` una sola vez. DeliTools almacena únicamente un hash de verificación y no puede volver a mostrar el secreto. Si pierdes una clave, crea una nueva. Guarda el secreto en un gestor de secretos.
- **Caducidad.** `expiresAt` es opcional. Si lo envías, debe ser un momento futuro; de lo contrario, `CreateKey` devuelve un error de entrada no válida. Una clave sin caducidad sigue siendo válida hasta que la revoques. Una clave personal también requiere que su propietario conserve el acceso al tenant. Una clave de servicio no depende del usuario que la creó, por lo que eliminar a ese usuario no la detiene: revoca la clave de servicio o asígnale una caducidad.
- **Revocación.** Después de revocar una clave, o después de que el usuario de una clave personal o de un token de inicio de sesión pierda el permiso, las nuevas solicitudes se rechazan y una llamada en curso no realiza más intentos de reintento a la IA. Una llamada que ya está en curso puede terminar. Su uso se factura.

## Facturación

DeliTools informa del uso a Delcore. Delcore lo tarifica y lo liquida en USD.

- **Base.** La base son los precios públicos por token del modelo en el catálogo de modelos de Vercel AI Gateway, multiplicados por los tokens que la llamada midió realmente. Los descuentos de Gateway y los créditos promocionales no reducen la base.
- **Recargo.** La traducción cuesta la base multiplicada por 1.15. La extracción cuesta la base multiplicada por 1.30. Los importes utilizan decimales exactos y las reglas de redondeo de Delcore.
- **Cambios de precio.** DeliTools actualiza automáticamente los precios del catálogo. Un precio nuevo se aplica a todos los clientes en las solicitudes que comienzan después de que entre en vigor. Una solicitud conserva el precio de su inicio durante todos sus reintentos. Si DeliTools no tiene precio para un modelo, rechaza las nuevas ejecuciones de IA.
- **Los fallos son facturables.** Los tokens utilizados por reintentos, llamadas fallidas, tiempos de espera agotados, cancelaciones y resultados que no pudiste recibir se siguen contando, junto con el recargo.
- **Uso desconocido.** Si DeliTools no puede leer de inmediato el uso de una llamada, aplaza el cargo y lo concilia con Gateway más tarde. Nunca estima el uso para un cargo. Si el uso sigue siendo desconocido cuando el registro caduca a los 30 días, ese cargo se descarta.
- **Dónde ver el uso.** Las respuestas no contienen el uso. Utiliza la interfaz de facturación de Delino para tu tenant para revisar el uso y los cargos.
- **Tenants bloqueados.** Si la facturación bloquea tu tenant, las nuevas ejecuciones fallan con un error de facturación bloqueada. Un intento de IA que ya está en curso no se interrumpe, pero DeliTools vuelve a comprobar la autorización antes de cada reintento. Si para entonces la facturación ha bloqueado el tenant, no realiza más intentos y devuelve el error de facturación bloqueada. Como DeliTools mide el uso después de la ejecución, las llamadas simultáneas pueden superar un saldo o un límite antes de que se aplique el bloqueo.

## Idempotencia

`idempotencyKey` es opcional. Si la envías, debe tener de 1 a 256 caracteres ASCII visibles (sin espacios ni caracteres de control); cualquier otra clave devuelve un error de entrada no válida. Sin ella, cada llamada es una ejecución nueva y puede generar un cargo nuevo, incluso cuando reintentas tras un fallo de red.

Con una clave, DeliTools hace el seguimiento de la solicitud por **tenant, método y clave**. Esto se aplica a distintas credenciales del mismo tenant. DeliTools comprueba tu autorización actual antes de informarte del estado de una solicitud.

| Situación                                             | Resultado                                                                               |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Clave nueva                                           | La llamada se ejecuta con normalidad.                                                   |
| Misma clave y misma entrada, aún en ejecución         | Un error de procesamiento con el ID de solicitud de la primera llamada. No espera.      |
| Misma clave y misma entrada, finalizada               | Un error de ya procesado. El resultado no se reproduce.                                 |
| Misma clave y entrada distinta                        | Un error de conflicto de entrada.                                                       |
| La primera llamada terminó en un estado desconocido   | Un error de resultado desconocido. DeliTools no repite por sí mismo la llamada a la IA. |
| Misma clave después de caducar el registro de 30 días | Una llamada nueva, con cargo normal.                                                    |

DeliTools conserva la clave, un hash de la entrada y el estado. No conserva la entrada ni el resultado. La deduplicación solo se mantiene durante 30 días después de la primera llamada. Guarda el resultado cuando lo recibas: una llamada repetida con la misma clave no puede devolverlo de nuevo.

## Errores

Cada error que devuelve un método tiene un código de Connect, un motivo estable, un ID de solicitud y un mensaje en inglés. El motivo y el ID de solicitud están en el detalle de error `delitools.v1.ErrorDetail`. En tu código, utiliza el motivo, no el mensaje. Los errores que el transporte de Connect genera antes de que se ejecute un método, por ejemplo un cuerpo de solicitud mal formado, un tipo de contenido no admitido, un mensaje demasiado grande o un método desconocido, tienen un código de Connect y un mensaje, pero no `ErrorDetail`. El ID de solicitud sigue estando en el encabezado de respuesta `X-Delitools-Request-Id`, que incluye cada respuesta. Lee el motivo de `ErrorDetail` solo cuando esté presente.

| Motivo                               | Código de Connect     | Significado                                                                                                                       |
| ------------------------------------ | --------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | La solicitud no es válida, por ejemplo una matriz vacía o idiomas idénticos.                                                      |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | El esquema está fuera del subconjunto compatible.                                                                                 |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | El modelo no está en la lista de permitidos.                                                                                      |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | La credencial falta, no es válida, ha caducado o se ha revocado.                                                                  |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | La credencial o el usuario no puede utilizar esta herramienta en este tenant.                                                     |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | La facturación no permite nuevas ejecuciones de pago para el tenant.                                                              |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | No hay un precio público disponible para el modelo, por lo que DeliTools no lo ejecuta.                                           |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | Se superó un límite configurado, como el tamaño de entrada, los elementos, la frecuencia o la simultaneidad.                      |
| `ERROR_REASON_PROCESSING`            | `aborted`             | Una solicitud con la misma clave de idempotencia sigue en ejecución.                                                              |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | Una solicitud con la misma clave de idempotencia y la misma entrada ya finalizó.                                                  |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | La clave de idempotencia se utilizó con una entrada distinta.                                                                     |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | Se desconoce el resultado de la primera ejecución.                                                                                |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | El texto no contiene información que el esquema exige.                                                                            |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | El modelo devolvió una salida mal formada o una salida que no coincide con el esquema.                                            |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | El proveedor de IA falló después de los reintentos permitidos.                                                                    |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | Se superó el tiempo de espera de ejecución.                                                                                       |
| `ERROR_REASON_CANCELED`              | `canceled`            | Se canceló la llamada.                                                                                                            |
| `ERROR_REASON_INTERNAL`              | `internal`            | Un fallo interno, por ejemplo en el almacenamiento.                                                                               |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | No se pudo completar la comprobación de autorización o de facturación del tenant, por lo que se rechaza el nuevo trabajo de pago. |

Gestión:

- Corrige la solicitud en los errores `invalid_argument`, `permission_denied`, `unauthenticated` y `failed_precondition` antes de reintentar. Resuelve el bloqueo de facturación en un error de facturación bloqueada. La excepción es `ERROR_REASON_PRICE_UNAVAILABLE`. Describe el estado de la implementación: DeliTools rechaza la llamada antes de iniciar ningún trabajo o de guardar la clave de idempotencia, y la misma solicitud sin cambios puede funcionar más tarde, cuando se recupere el catálogo de precios.
- Un error de proveedor, de tiempo de espera o interno se puede reintentar más tarde. Los fallos de proveedor y de tiempo de espera aún pueden facturarse. Utiliza una clave de idempotencia para evitar una segunda ejecución cuando no sepas si la primera llamada llegó a DeliTools, por ejemplo tras un fallo de red. Cuando una llamada con clave ha terminado con un error, como un error de proveedor, la misma clave devuelve `ERROR_REASON_ALREADY_PROCESSED` (o `ERROR_REASON_UNKNOWN_OUTCOME` si se desconoce su resultado) y no inicia una nueva ejecución. Para repetir el trabajo a propósito, envía una clave nueva. La nueva ejecución puede generar otro cargo. Un error `unavailable` con `ERROR_REASON_INTERNAL` significa que DeliTools no pudo completar la comprobación de autorización o de facturación. Evalúa el código y el motivo juntos, porque `ERROR_REASON_INTERNAL` también aparece con `internal`.
- Los errores de resultado desconocido, de procesamiento y de ya procesado contienen el ID de solicitud de la primera llamada. Contacta con asistencia con ese ID si el resultado sigue siendo desconocido.

## Retención de datos

DeliTools nunca almacena tu texto de entrada, tu esquema ni los resultados. También los mantiene fuera de los registros, las trazas y los eventos de auditoría. Las credenciales y los secretos se excluyen de la telemetría.

DeliTools almacena solo los datos que necesita para ejecutar el servicio y facturarlo: hashes de verificación de las claves de API, quién y qué tenant es el propietario de una solicitud, hashes de entrada para la idempotencia, estados de las solicitudes, identificadores de intentos, uso de tokens medido, instantáneas de precios y registros seguros de auditoría y operación.

| Datos                                                          | Se conservan durante                             |
| -------------------------------------------------------------- | ------------------------------------------------ |
| Registros de idempotencia                                      | 30 días desde la primera creación                |
| Registros de entrega y conciliación del uso                    | 30 días desde la primera creación                |
| Registros de auditoría de la plataforma y registros operativos | 30 días desde la creación                        |
| Claves de API                                                  | No se eliminan con la retención de 30 días       |
| Asientos contables aceptados en Delcore                        | Según las reglas de retención propias de Delcore |

La retención de 30 días no se aplica a las claves de API. La limpieza de retención nunca las elimina. La caducidad y la revocación solo impiden que una clave se autentique, por lo que su hash de verificación y su atribución siguen almacenados.

Los 30 días comienzan cuando el registro se crea por primera vez. Los reintentos y los cambios de estado no los amplían. DeliTools elimina un registro cuando caduca en cualquier estado, incluidas las solicitudes que no finalizaron y el uso que aún no se ha entregado. Aceptas dos riesgos:

- Después de que caduque un registro de idempotencia, una llamada repetida con la misma clave se ejecuta y se cobra de nuevo.
- El uso que sigue pendiente cuando caduca su registro se pierde y DeliTools no lo cobra.

## Tratamiento de datos por proveedores externos

Para realizar su trabajo, DeliTools envía tu texto, y en la extracción tu esquema, a Vercel AI Gateway y, desde allí, al proveedor del modelo que ejecuta tu solicitud. Estos servicios tratan los datos conforme a sus propias políticas. Léelas antes de enviar datos a DeliTools:

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models): la página de cada modelo enumera sus proveedores.
- La política de datos del proveedor del modelo que utilices.

DeliTools no promete cero retención de datos en ningún proveedor. No detecta datos confidenciales y no los elimina de tu entrada. No envíes datos que no puedas compartir con estos proveedores.

## Límites

El operador del servidor de API de DeliTools establece los límites del tamaño de entrada, del número de elementos de una traducción, del tiempo de espera de ejecución, de las solicitudes por minuto por tenant, de las solicitudes simultáneas por tenant, del número de reintentos de IA y de la lista de modelos de traducción permitidos. Son ajustes de la implementación y pueden cambiar. No son límites fijos del producto y DeliTools no ofrece ninguna garantía de rendimiento ni de disponibilidad. Una solicitud que supera un límite devuelve `ERROR_REASON_LIMIT`; la excepción es una llamada que supera el tiempo de espera de ejecución, que devuelve `ERROR_REASON_TIMEOUT`. En el caso de un modelo no válido, devuelve `ERROR_REASON_INVALID_MODEL`. Pregunta al operador de tu implementación por sus valores actuales.
