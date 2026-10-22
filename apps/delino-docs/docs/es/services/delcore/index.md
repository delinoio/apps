# Delcore

Delcore proporciona tenants compartidos, control de acceso y facturación para los servicios de Delino. El servidor oficial de la API es `https://delcore.api.delino.io`. Esta página describe los precios por uso, la caducidad de los registros y la comprobación del estado de los tenants para los desarrolladores de servicios.

## Comprobar el estado de un tenant

Un servicio registrado puede consultar a Delcore si un tenant está activo, bloqueado o eliminado. Utiliza esta comprobación cuando tu servicio almacene datos de un tenant, pero no disponga de un usuario de Delino para cada solicitud; por ejemplo, cuando la llamada se realice con una clave de API de un proyecto.

Llama a `delcore.v1.DelcoreService/GetTenantStates` con entre 1 y 100 ID de tenant. Cada ID debe ser un UUID de versión 7 en minúsculas. Autentícate con el token de acceso de la aplicación máquina a máquina de Logto de tu servicio. El servicio debe estar registrado en Delcore y tener el ámbito `authorize`. Los tokens de usuario, los servicios no registrados y los servicios sin este ámbito no pueden consultar el estado de los tenants.

La respuesta repite cada ID de tenant solicitado, en el orden en que los enviaste, con un estado:

| Estado                  | Significado                                                        |
| ----------------------- | ------------------------------------------------------------------ |
| `TENANT_STATE_ACTIVE`   | El tenant existe y no está bloqueado ni en proceso de eliminación. |
| `TENANT_STATE_BLOCKED`  | Un operador de la plataforma ha bloqueado el tenant.               |
| `TENANT_STATE_DELETING` | La eliminación está pendiente.                                     |
| `TENANT_STATE_DELETED`  | La eliminación es definitiva.                                      |
| `TENANT_STATE_UNKNOWN`  | Delcore no tiene ningún tenant con este ID.                        |

Cuando se cumple más de una condición, `DELETED` prevalece sobre `DELETING`, `DELETING` prevalece sobre `BLOCKED` y `BLOCKED` prevalece sobre `ACTIVE`.

El estado no depende de la facturación. Una suscripción caducada, una renovación fallida, una disputa o un saldo bajo no hacen que un tenant pase a `BLOCKED`. Para decidir si un usuario puede ejecutar tareas de pago, utiliza `Authorize` en su lugar. La comprobación solo lee datos. No modifica ningún tenant, no autoriza el uso de pago ni contacta con un proveedor de pagos, y no devuelve nombres de tenants, datos de usuarios ni información de facturación.

## Gestionar resultados y errores

- Permite el acceso solo para `TENANT_STATE_ACTIVE`, o cuando tus propias reglas documentadas para propietarios o administradores permitan el acceso a un tenant bloqueado o en proceso de eliminación.
- Trata `TENANT_STATE_UNKNOWN`, los errores de solicitud, los tiempos de espera agotados y la falta de disponibilidad de Delcore como una denegación de acceso. No elimines datos en estos casos.
- Para la limpieza motivada por la eliminación de un tenant, elimina automáticamente los datos del tenant que conserves solo después de recibir un estado explícito `TENANT_STATE_DELETED`. `BLOCKED` y `DELETING` nunca permiten esa limpieza. La caducidad fija de los registros se aplica independientemente del estado del tenant.
- Los ID no válidos, una solicitud vacía o más de 100 ID devuelven un error de argumento no válido. Las credenciales ausentes, caducadas o no válidas devuelven un error de falta de autenticación. Quien realice la llamada sin tener permiso para consultar el estado de los tenants recibirá un error de permiso denegado. Otros fallos devuelven un error con un ID de correlación; incluye este ID, que se encuentra en los detalles del error y en la cabecera `X-Delcore-Correlation-Id`, cuando solicites asistencia.
- Reintenta más tarde las solicitudes fallidas. Una solicitud fallida nunca significa que un tenant esté activo o eliminado.

## Precios por uso y caducidad de los registros

Los precios unitarios por uso se aplican en el momento del inicio de la ejecución inicial para todos los clientes, incluidos los suscriptores existentes. Informa de ese inicio en `IngestEvent` como `occurred_at` y mantenlo en los reintentos de ejecución y en la conciliación del uso. Los cambios de precio se aplican a las ejecuciones posteriores; las entregas tardías y las correcciones mantienen la atribución del precio original. Los precios de los pagos recurrentes, las funciones seleccionadas y las asignaciones de uso incluido conservan las condiciones de su suscripción.

La deduplicación de solicitudes dura 30 días desde la creación inicial del registro. Un reintento no amplía este plazo. Tras la caducidad, la misma clave de solicitud puede iniciar una nueva operación sujeta a los permisos vigentes y a comprobaciones independientes de la identidad del pago o del evento. Los eventos de uso aceptados conservan sus identidades persistentes para evitar efectos contables duplicados.

Los servicios consumidores deben eliminar los registros de entrega y conciliación del uso 30 días después de su creación inicial, incluso cuando la ejecución o el uso sigan siendo desconocidos o un informe no haya llegado a Delcore. Reintenta la entrega y la consulta del uso desconocido hasta que se alcance esa caducidad y, después, abandona el cargo pendiente. No estimes el uso para la facturación. Esto puede provocar la pérdida permanente de uso facturable, y una clave de deduplicación de ejecución caducada puede permitir una nueva ejecución y un nuevo cargo. Los registros de auditoría de la plataforma y los registros operativos también caducan 30 días después de su creación. Estos límites no eliminan los asientos contables ya aceptados por Delcore ni modifican la validez de las claves de API.
