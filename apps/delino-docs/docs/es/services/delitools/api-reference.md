# Referencia de la API de DeliTools

Esta página describe los dos métodos de DeliTools. Ambos son procedimientos de Connect en `delitools.v1.DelitoolsService` y aceptan JSON mediante HTTP `POST`. Los nombres de los campos usan lowerCamelCase en JSON. Envía `Content-Type: application/json` y `Authorization: Bearer <credential>`. Consulta [DeliTools](/es/services/delitools/) para obtener información sobre credenciales y tenants.

Cada respuesta incluye un ID de solicitud, tanto en el cuerpo como en la cabecera `X-Delitools-Request-Id`. Las respuestas no incluyen el uso de tokens ni los cargos. Consulta esos datos en Delcore; consulta [Facturación](/es/services/delitools/billing).

Cada llamada es una nueva ejecución. DeliTools no tiene clave de idempotencia ni resultados almacenados. Si repites una solicitud, se ejecuta de nuevo y puede volver a generar cargos.

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| Campo            | Tipo             | Reglas                                                                                                                |
| ---------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | cadena           | Obligatorio para un token de inicio de sesión de Delino. Para una clave de API, vacío o el tenant de la clave.        |
| `texts`          | array de cadenas | Obligatorio. Al menos un elemento. Ninguna cadena vacía. El operador limita el número de elementos y el tamaño total. |
| `targetLanguage` | cadena           | Obligatorio. Una etiqueta de idioma BCP 47, como `fr` o `pt-BR`.                                                      |
| `sourceLanguage` | cadena           | Opcional. Una etiqueta de idioma BCP 47. Si se omite, el modelo detecta el idioma de origen.                          |
| `model`          | cadena           | Obligatorio. Un identificador de modelo que el operador permita.                                                      |

La respuesta contiene `requestId` y `translations`. `translations` tiene el mismo número de elementos y en el mismo orden que `texts`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"sourceLanguage":"en","targetLanguage":"fr","model":"<model-id>"}'
```

```json
{
  "requestId": "<request-id>",
  "translations": ["Bonjour", "Bonjour, bonne journée"]
}
```

- **Etiquetas de idioma.** DeliTools comprueba que la etiqueta esté bien formada según BCP 47, sin distinguir entre mayúsculas y minúsculas, con un máximo de 64 caracteres y un idioma principal de dos o tres letras. Se rechazan las etiquetas formadas únicamente por subetiquetas de uso privado y las etiquetas heredadas. DeliTools no comprueba que el modelo admita el idioma.
- **Todo o nada.** El resultado es atómico. Si falla algún elemento, falla toda la solicitud y no recibes traducciones parciales.
- **Sin casos especiales.** La omisión del idioma de origen, las entradas con varios idiomas y el texto que ya está en el idioma de destino se envían al modelo. DeliTools no los rechaza ni copia por ti el texto de origen.
- **Modelo.** Solo funcionan los modelos que el operador permita y que tengan precios registrados en Delcore. Nunca se sustituyen por otro modelo. Pregunta al operador del servicio qué identificadores de modelo están disponibles.

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| Campo        | Tipo   | Reglas                                                                                                                            |
| ------------ | ------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`   | cadena | Obligatorio para un token de inicio de sesión de Delino. Para una clave de API, vacío o el tenant de la clave.                    |
| `text`       | cadena | Obligatorio y no vacío. El operador limita el tamaño.                                                                             |
| `jsonSchema` | cadena | Obligatorio. Un documento JSON Schema, como cadena JSON, que use únicamente el subconjunto admitido que se indica a continuación. |

El modelo está fijado en `openai/gpt-6-luna`. No puedes elegirlo. La respuesta contiene `requestId` y `json`, una cadena JSON que se ajusta a tu esquema.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Extract" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"text":"Please send three boxes of green tea.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"},\"quantity\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"item\",\"quantity\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "<request-id>",
  "json": "{\"item\":\"green tea\",\"quantity\":3}"
}
```

- **Basado en tu texto.** El modelo debe tomar los valores del texto. Si el texto carece de información obligatoria, la solicitud falla con `ERROR_REASON_EXTRACTION_FAILED`. DeliTools no inventa valores.
- **Salida verificada.** DeliTools valida el resultado con tu esquema. Una salida mal formada o que no se ajuste al esquema falla con `ERROR_REASON_INVALID_OUTPUT`. DeliTools no vuelve a intentarlo para corregir una salida no válida.
- **Exactitud.** Un resultado que se ajuste al esquema puede seguir siendo incorrecto. Comprueba los resultados importantes.
- **Los esquemas no admitidos fallan antes.** DeliTools rechaza los esquemas que quedan fuera del subconjunto admitido antes de ejecutar el modelo, por lo que esas solicitudes no generan cargos.

### Subconjunto de JSON Schema admitido

El esquema se interpreta como JSON Schema 2020-12. Debe ser un objeto JSON. `$schema` es opcional y solo se permite en la raíz, donde DeliTools ignora su valor. Cada esquema, incluidos la raíz, el esquema de cada propiedad y el esquema de `items`, necesita `type`, `enum` o `const`. Un esquema puede tener como máximo 8 niveles de profundidad y 256 nodos de esquema, y el operador limita su tamaño.

| Palabra clave            | Regla                                                                                                                                                                              |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`, o un array de estos tipos, por ejemplo `["string","null"]` para un valor que admita nulos.                    |
| `properties`, `required` | `properties` asocia nombres a esquemas. Cada nombre de `required` debe aparecer en `properties`. Como máximo 64 propiedades por objeto, y los nombres de `required` no se repiten. |
| `additionalProperties`   | Solo `true` o `false` (no un esquema).                                                                                                                                             |
| `items`                  | Un esquema para todos los elementos.                                                                                                                                               |
| `enum`, `const`          | `enum` tiene de 1 a 64 valores distintos. `const` es un único valor.                                                                                                               |
| `minLength`, `maxLength` | Enteros de 0 a 1.000.000, para cadenas. El mínimo no puede superar el máximo.                                                                                                      |
| `minItems`, `maxItems`   | Enteros de 0 a 1.000.000, para arrays. El mínimo no puede superar el máximo.                                                                                                       |
| `minimum`, `maximum`     | Números finitos, para `number` e `integer`. El mínimo no puede superar el máximo.                                                                                                  |
| `title`, `description`   | Notas para el modelo. No modifican la validación.                                                                                                                                  |

Los siguientes elementos no se admiten y devuelven `ERROR_REASON_UNSUPPORTED_SCHEMA`: `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, las palabras clave `unevaluated`, los esquemas booleanos, las palabras clave desconocidas y los esquemas demasiado grandes o con demasiados niveles de anidación. DeliTools nunca sigue referencias, por lo que un esquema no puede hacer que obtenga nada.

Establece `"additionalProperties": false` en los objetos cuando quieras únicamente las propiedades que has enumerado.

## Errores

Los errores usan un código estándar de Connect. Los detalles del error incluyen un motivo estable y el ID de solicitud. Los mensajes están en inglés y nunca contienen tu contenido.

| Motivo                                | Código de Connect                           | Significado                                                                                                                                                                                                               |
| ------------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | Un campo no es válido: array o cadena vacíos, etiqueta de idioma incorrecta, tenant ausente o mal formado, modelo ausente o no permitido, o un esquema que no es JSON válido.                                             |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | La credencial falta, está mal formada, ha caducado, se ha revocado o es desconocida.                                                                                                                                      |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | La credencial carece de la capacidad, de la concesión de la clave o de tu permiso en el tenant, o el `tenantId` que enviaste no es el tenant de la clave.                                                                 |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | Delcore bloqueó el uso de pago, por ejemplo debido al saldo, una renovación, una disputa, un bloqueo del operador o un tenant que se está eliminando.                                                                     |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | La entrada, el número de elementos o el esquema superan los límites del operador.                                                                                                                                         |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | Tu tenant alcanzó su límite de frecuencia de solicitudes o de concurrencia. Vuelve a intentarlo más tarde. Un error de frecuencia de solicitudes incluye una cabecera `Retry-After`.                                      |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` or `failed_precondition` | El modelo no está permitido (`invalid_argument`), o no tiene precios de entrada y salida registrados (`failed_precondition`). No se sustituye por otro modelo.                                                            |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | El esquema usa una palabra clave fuera del subconjunto admitido.                                                                                                                                                          |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` or `internal`                 | El proveedor de IA siguió fallando tras los reintentos permitidos (`unavailable`), o rechazó la solicitud (`internal`).                                                                                                   |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | La salida del modelo estaba mal formada, se truncó o no cumplía el contrato.                                                                                                                                              |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | El texto no contiene la información obligatoria.                                                                                                                                                                          |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | Se agotó el tiempo de espera de ejecución del operador. Abarca las comprobaciones y todos los intentos.                                                                                                                   |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | La solicitud se canceló o se cerró la conexión.                                                                                                                                                                           |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | No se pudo completar una comprobación de autorización, precios o facturación, o DeliTools no pudo registrar el uso de la llamada. El uso de pago se bloquea por seguridad en caso de fallo y no recibes ningún resultado. |

Una solicitud que supere el límite global de tamaño de solicitud del operador puede recibir un error `resource_exhausted` simple, sin estos detalles. Consulta [Solución de problemas](/es/services/delitools/troubleshooting) para saber qué hacer.

## Límites y reintentos

El operador configura estos límites, que pueden cambiar: tamaño de entrada, número de elementos de traducción, tamaño y profundidad del esquema, tiempo de espera de ejecución, frecuencia y ráfaga de solicitudes por tenant, ejecuciones simultáneas por tenant y número máximo de reintentos ante fallos transitorios. Ponte en contacto con el operador del servicio para conocer los valores que se te aplican. Un límite es una protección, no una garantía de rendimiento.

- DeliTools solo reintenta los fallos transitorios del proveedor de IA, como errores de red, errores del servidor y respuestas de límite de frecuencia, hasta el número configurado. Nunca reintenta para corregir una salida no válida.
- Antes de cada reintento, DeliTools vuelve a comprobar tu autorización. Si tu clave se revocó o caducó, perdiste el permiso o la facturación está bloqueada, DeliTools se detiene y devuelve el error correspondiente. Una llamada al proveedor que ya esté en ejecución puede finalizar.
- Si cancelas la solicitud o cierras la conexión, DeliTools cancela la llamada al proveedor y deja de reintentar.
- Si tu cliente cierra la llamada o su propio tiempo de espera la termina, DeliTools lo trata como una cancelación.
- El tiempo de espera de ejecución del operador abarca la autorización, la validación y todos los intentos. Cuando se agota, DeliTools aborta la llamada al proveedor y devuelve `ERROR_REASON_TIMEOUT`.
