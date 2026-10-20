# Delcore

Delcore proporciona inquilinos compartidos, control de acceso y facturación para los servicios de Delino. El servidor oficial de la API es `https://delcore.api.delino.io`. Esta página describe la comprobación del estado de los inquilinos para desarrolladores de servicios.

## Comprobar el estado de un inquilino

Un servicio registrado puede preguntar a Delcore si un inquilino está activo, bloqueado o eliminado. Utilice esta comprobación cuando su servicio almacene datos de un inquilino, pero no disponga de un usuario de Delino para cada solicitud, por ejemplo, cuando la llamada se realice con una clave de API de proyecto.

Llame a `delcore.v1.DelcoreService/GetTenantStates` con entre 1 y 100 identificadores de inquilino. Cada identificador debe ser un UUID versión 7 en minúsculas. Autentíquese con el token de acceso de la aplicación de máquina a máquina de Logto de su servicio. El servicio debe estar registrado en Delcore y tener el ámbito `authorize`. Los tokens de usuario, los servicios no registrados y los servicios sin este ámbito no pueden consultar el estado de los inquilinos.

La respuesta repite cada identificador de inquilino solicitado, en el orden en que se envió, con uno de los siguientes estados:

| Estado                  | Significado                                                           |
| ----------------------- | --------------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | El inquilino existe y no está bloqueado ni en proceso de eliminación. |
| `TENANT_STATE_BLOCKED`  | Un operador de la plataforma bloqueó al inquilino.                    |
| `TENANT_STATE_DELETING` | La eliminación está pendiente.                                        |
| `TENANT_STATE_DELETED`  | La eliminación es definitiva.                                         |
| `TENANT_STATE_UNKNOWN`  | Delcore no tiene ningún inquilino con este identificador.             |

Cuando se aplica más de una condición, `DELETED` prevalece sobre `DELETING`, `DELETING` prevalece sobre `BLOCKED` y `BLOCKED` prevalece sobre `ACTIVE`.

El estado no depende de la facturación. Una suscripción vencida, una renovación fallida, una disputa o un saldo bajo no hacen que un inquilino pase a estar `BLOCKED`. Para decidir si un usuario puede ejecutar trabajo de pago, utilice `Authorize` en su lugar. La comprobación solo lee datos. No modifica un inquilino, no autoriza el uso de pago ni contacta con un proveedor de pagos, y no devuelve nombres de inquilinos, datos de usuarios ni información de facturación.

## Gestionar los resultados y los errores

- Permita el acceso únicamente para `TENANT_STATE_ACTIVE`, o cuando sus propias reglas documentadas para propietarios o administradores permitan acceder a un inquilino bloqueado o en proceso de eliminación.
- Trate `TENANT_STATE_UNKNOWN`, los errores de solicitud, los tiempos de espera agotados y la falta de disponibilidad de Delcore como denegación de acceso. No elimine datos en estos casos.
- Elimine automáticamente los datos de inquilino que conserve únicamente después de recibir explícitamente `TENANT_STATE_DELETED`. `BLOCKED` y `DELETING` nunca permiten la eliminación automática.
- Los identificadores no válidos, una solicitud vacía o más de 100 identificadores devuelven un error de argumento no válido. Las credenciales ausentes, vencidas o no válidas devuelven un error de falta de autenticación. Un solicitante que no tenga permiso para consultar el estado de los inquilinos recibe un error de permiso denegado. Otros fallos devuelven un error con un identificador de correlación; incluya este identificador, que se encuentra en los detalles del error y en el encabezado `X-Delcore-Correlation-Id`, cuando solicite asistencia.
- Vuelva a intentar una solicitud fallida más tarde. Una solicitud fallida nunca significa que un inquilino esté activo o eliminado.
