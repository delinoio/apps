# DeliTools

DeliTools ofrece dos API de pago para desarrolladores: traducción de una lista de textos y extracción de un valor JSON a partir de texto. Puedes llamarlas con cualquier cliente de Connect o con JSON simple a través de HTTP desde tu propio código del lado del servidor. Delcore gestiona tus claves de API, permisos y facturación, para que uses la misma cuenta de Delino y el mismo tenant para todo.

## Conceptos

- **Traducción:** envía una lista de textos, un idioma de destino y un modelo. Recibes las traducciones en el mismo orden y en la misma cantidad.
- **Extracción:** envía un texto y un JSON Schema. Recibes un documento JSON que se ajusta al esquema.
- **Tenant:** un espacio personal o una organización de Delino. Delcore factura el uso al tenant.
- **Credencial:** un token de inicio de sesión de Delino, una clave de API personal o una clave de API de servicio del tenant. Consulta [Claves de API](/es/services/delitools/api-keys).
- **ID de solicitud:** cada respuesta incluye un ID de solicitud. Inclúyelo cuando solicites asistencia.

## Inicio de sesión y permisos

Envía una cabecera `Authorization: Bearer <credential>` con cada solicitud.

| Credencial                          | Formato                           | Tenant                                                |
| ----------------------------------- | --------------------------------- | ----------------------------------------------------- |
| Token de inicio de sesión de Delino | Un token de acceso de Delino      | Establece `tenantId` en la solicitud. Es obligatorio. |
| Clave de API personal               | Un secreto que empieza por `dlk_` | El tenant de la clave. `tenantId` es opcional.        |
| Clave de servicio del tenant        | Un secreto que empieza por `dlk_` | El tenant de la clave. `tenantId` es opcional.        |

Si estableces `tenantId` junto con una clave de API, debe ser el tenant de esa clave; de lo contrario, la solicitud falla con `ERROR_REASON_PERMISSION_DENIED`. Delcore debe autorizar cada solicitud en ese momento: la credencial necesita la capacidad `translate` o `extract`, el tenant debe estar activo y debe superar las comprobaciones habituales de facturación. Estas API son de pago y Delcore las bloquea cuando tu tenant tiene un problema de facturación, como un saldo agotado.

## Inicio rápido

1. **Obtén una credencial.** Usa tu token de inicio de sesión de Delino o crea una clave de API como se describe en [Claves de API](/es/services/delitools/api-keys).
2. **Busca tu ID de tenant y un modelo.** El ID de tenant forma parte de tu cuenta de Delino. El operador del servicio establece qué modelos de traducción están disponibles.
3. **Llama a la API.** Todos los procedimientos están en `delitools.v1.DelitoolsService`. Establece `DELITOOLS_URL` en la URL base del servidor de la API de DeliTools, por ejemplo, `http://127.0.0.1:46322` para un servidor local.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

En JSON, los nombres de los campos usan lowerCamelCase. Consulta la [Referencia de la API](/es/services/delitools/api-reference) para ver todos los campos, el subconjunto de esquemas compatible y los motivos de error.

Llama a DeliTools desde código del lado del servidor o desde la línea de comandos. No incluyas una clave de API en el código del frontend: DeliTools no admite llamadas desde navegadores.

## Tu contenido y el proveedor de IA

DeliTools ejecuta tu solicitud en Vercel AI Gateway. **Los textos y esquemas que envías se envían a Vercel AI Gateway y al proveedor del modelo que elijas.** El tratamiento que hacen de tu contenido, incluida cualquier retención, se rige por sus propias condiciones y configuraciones, no por las de DeliTools.

- DeliTools no almacena tus textos, esquemas ni resultados, ni los escribe en sus registros. Esto describe únicamente a DeliTools. No es una garantía sobre lo que conserva Vercel AI Gateway o el proveedor del modelo.
- No envíes contenido que no tengas permitido compartir con esos proveedores. Revisa sus condiciones antes de enviar datos sensibles o regulados.
- DeliTools conserva únicamente los registros de uso que necesita para facturar a tu tenant y los elimina 30 días después de su creación.
- No se garantiza que la salida del modelo sea correcta. Comprueba los resultados antes de basarte en ellos.

## Más guías

- [Referencia de la API](/es/services/delitools/api-reference): campos, etiquetas de idioma, subconjunto de esquemas, errores y límites.
- [Claves de API](/es/services/delitools/api-keys): claves personales y de tenant, permisos, caducidad y revocación.
- [Facturación](/es/services/delitools/billing): cómo se convierte el uso de tokens en cargos.
- [Solución de problemas](/es/services/delitools/troubleshooting): configuración del cliente y resolución de errores.
