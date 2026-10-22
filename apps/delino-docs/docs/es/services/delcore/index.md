# Delcore

Delcore proporciona tenants compartidos, control de acceso y facturación para los servicios de Delino. El servidor oficial de la API es `https://delcore.api.delino.io`. Esta página describe la verificación de claves de API, los precios por uso, la caducidad de los registros y la comprobación del estado del tenant para los desarrolladores de servicios.

## Comprobar el estado del tenant

Un servicio registrado puede consultar a Delcore si un tenant está activo, bloqueado o eliminado. Utiliza esta comprobación cuando tu servicio almacene datos de un tenant pero no disponga de un usuario de Delino para cada solicitud, por ejemplo, cuando la llamada se realice con una clave de API de proyecto.

Llama a `delcore.v1.DelcoreService/GetTenantStates` con entre 1 y 100 ID de tenant. Cada ID debe ser un UUID de versión 7 en minúsculas. Autentícate con el token de acceso de la aplicación de máquina a máquina de Logto de tu servicio. El servicio debe estar registrado en Delcore y tener el ámbito `authorize`. Los tokens de usuario, los servicios no registrados y los servicios sin este ámbito no pueden consultar el estado del tenant.

La respuesta repite cada ID de tenant solicitado, en el orden en que los enviaste, con un estado:

| Estado                  | Significado                                                        |
| ----------------------- | ------------------------------------------------------------------ |
| `TENANT_STATE_ACTIVE`   | El tenant existe y no está bloqueado ni en proceso de eliminación. |
| `TENANT_STATE_BLOCKED`  | Un operador de la plataforma ha bloqueado el tenant.               |
| `TENANT_STATE_DELETING` | La eliminación está pendiente.                                     |
| `TENANT_STATE_DELETED`  | La eliminación es definitiva.                                      |
| `TENANT_STATE_UNKNOWN`  | Delcore no tiene ningún tenant con este ID.                        |

Cuando se aplica más de una condición, `DELETED` tiene prioridad sobre `DELETING`, `DELETING` tiene prioridad sobre `BLOCKED` y `BLOCKED` tiene prioridad sobre `ACTIVE`.

El estado no depende de la facturación. Una suscripción caducada, una renovación fallida, una disputa o un saldo bajo no hacen que un tenant pase a `BLOCKED`. Para decidir si un usuario puede ejecutar trabajo de pago, utiliza `Authorize` en su lugar. La comprobación solo lee datos. No modifica un tenant, no autoriza el uso de pago ni contacta con un proveedor de pagos, y no devuelve nombres de tenants, datos de usuarios ni información de facturación.

## Gestionar resultados y fallos

- Permite el acceso únicamente con `TENANT_STATE_ACTIVE`, o cuando tus propias reglas documentadas para propietarios o administradores permitan acceder a un tenant bloqueado o en proceso de eliminación.
- Trata `TENANT_STATE_UNKNOWN`, los errores de solicitud, los tiempos de espera agotados y la falta de disponibilidad de Delcore como una denegación de acceso. No elimines datos en estos casos.
- Para la limpieza derivada de la eliminación de un tenant, elimina automáticamente los datos del tenant que conserves solo tras recibir un `TENANT_STATE_DELETED` explícito. `BLOCKED` y `DELETING` nunca permiten esa limpieza. La caducidad fija de los registros se aplica independientemente del estado del tenant.
- Los ID no válidos, una solicitud vacía o más de 100 ID devuelven un error de argumento no válido. Las credenciales ausentes, caducadas o no válidas devuelven un error de falta de autenticación. Un llamante que no tenga permitido consultar el estado del tenant recibe un error de permiso denegado. Los demás fallos devuelven un error con un ID de correlación; incluye este ID, que se encuentra en los detalles del error y en la cabecera `X-Delcore-Correlation-Id`, cuando solicites asistencia.
- Reintenta una solicitud fallida más tarde. Una solicitud fallida nunca significa que un tenant esté activo o eliminado.

## Precios por uso y caducidad de los registros

Los precios unitarios por uso se aplican en el momento del inicio de la ejecución inicial para todos los clientes, incluidos los suscriptores existentes. Notifica ese inicio en `IngestEvent` como `occurred_at` y mantenlo en los reintentos de ejecución y en la conciliación del uso. Un cambio de precio se aplica a las ejecuciones posteriores; las entregas tardías y las correcciones mantienen la atribución del precio original. Los precios de los pagos recurrentes, las funciones seleccionadas y las asignaciones de uso incluido conservan sus condiciones de suscripción.

La deduplicación de solicitudes dura 30 días desde la creación inicial del registro. Un reintento no amplía este periodo. Tras la caducidad, la misma clave de solicitud puede iniciar una nueva operación sujeta a los permisos actuales y a comprobaciones independientes de identidad del pago o del evento. Los eventos de uso aceptados conservan sus identidades persistentes para evitar efectos contables duplicados.

Los servicios consumidores deben eliminar los registros de entrega de uso y de conciliación 30 días después de su creación inicial, incluso cuando la ejecución o el uso sigan siendo desconocidos o un informe no haya llegado a Delcore. Reintenta la entrega y la consulta del uso desconocido hasta esa caducidad y, después, abandona el cargo pendiente. No estimes el uso para la facturación. Esto puede provocar la pérdida permanente de uso facturable, y una clave de deduplicación de ejecución caducada puede permitir una nueva ejecución y un nuevo cargo. Los registros de auditoría de la plataforma y los registros operativos también caducan 30 días después de su creación. Estos límites no eliminan los asientos contables ya aceptados por Delcore ni cambian la validez de las claves de API.

## Verificar una clave de API

Delcore emite claves de API reutilizables, tanto personales como de servicio del tenant, para cualquier servicio registrado. Los usuarios y los propietarios y administradores del tenant las crean con un token de inicio de sesión de Delino; consulta [Claves de API de DeliTools](/es/services/delitools/api-keys) para conocer el flujo de cara al usuario. Una clave empieza por `dlk_`, concede capacidades seleccionadas de servicios seleccionados y pertenece a un solo tenant.

Un servicio registrado verifica una clave que recibe de sus llamantes con `delcore.v1.DelcoreService/AuthorizeApiKey`. Autentícate con el token de acceso de máquina a máquina de Logto de tu servicio. El servicio debe estar registrado y tener el ámbito `authorize`.

- Envía el `secret` completo, la `capability` que se está utilizando y `paid` para el trabajo de pago. También puedes enviar un `feature_key` y `group_values`, como en `Authorize`.
- Delcore evalúa únicamente las concesiones que nombran al servicio llamante, por lo que tu servicio nunca puede autorizar las capacidades de otro servicio.
- Un secreto con formato incorrecto, desconocido, revocado o caducado, o el secreto de un tenant eliminado, devuelve un error de falta de autenticación y ningún detalle de la clave. Delcore también responde de esta manera cuando se rechaza el token de tu propio servicio, así que comprueba las credenciales de tu servicio si fallan todas las claves válidas. Trátalo como una credencial no válida.
- Para un secreto válido, la respuesta contiene `allowed`, `reasons` y el campo opcional `feature`, además de `keyId`, `tenantId`, `ownerKind` y `userId`. Una clave sin la capacidad devuelve `allowed: false` con `BLOCK_REASON_PERMISSION`.
- Una clave personal también necesita que su usuario tenga actualmente permiso para la capacidad en el tenant. Una clave de tenant no depende del usuario que la creó. Ambas necesitan un tenant activo y las mismas comprobaciones de facturación que `Authorize` cuando se establece `paid`.
- Compara el tenant que proporcione tu llamante, si lo hay, con el `tenantId` devuelto y rechaza cualquier discrepancia.
- Comprueba cada solicitud. No almacenes ningún resultado en caché. Trata los errores, los tiempos de espera agotados y la falta de disponibilidad de Delcore como una denegación de acceso para nuevo trabajo de pago. Un resultado de autorización no reserva nada.
