# DevBase

DevBase es un directorio gratuito de los usuarios finales de tus aplicaciones. Registras a tus propios usuarios, los consultas y los modificas mediante una API de Connect. Tus usuarios no necesitan cuentas de Delino y tu aplicación mantiene su propio inicio de sesión. DevBase proporciona a cada usuario un ID estable que puedes pasar a otros servicios.

## Conceptos

- **Tenant:** Un espacio personal u organización de Delino. Delcore administra los tenants y sus miembros.
- **Proyecto:** Un directorio con nombre de usuarios finales dentro de un tenant. Los proyectos no comparten usuarios, ni siquiera dentro del mismo tenant.
- **Usuario final:** Una persona o cuenta de tu aplicación. Tiene un ID externo que tú eliges, un ID interno que crea DevBase, un nombre y correo electrónico opcionales, y un objeto JSON de metadatos.
- **Clave de API del proyecto:** Un secreto que tu aplicación utiliza para leer o modificar los usuarios finales de un proyecto.

## Inicio de sesión y permisos

Envía un encabezado `Authorization: Bearer <token>` con cada solicitud.

- Los **desarrolladores** utilizan su token de inicio de sesión de Delino. Solo un Owner o Admin actual del tenant puede administrar proyectos y claves de API. También pueden leer y modificar usuarios finales con el token de inicio de sesión.
- Las **aplicaciones** utilizan una clave de API del proyecto. Una clave comienza por `dbk_`. Una clave `READ_ONLY` consulta y enumera usuarios. Una clave `CRUD` también crea, actualiza y elimina usuarios. Una clave nunca administra proyectos ni claves y solo funciona para su propio proyecto.

Los usuarios finales nunca inician sesión en DevBase. Sus registros no les proporcionan acceso a Delino.

## Inicio rápido

1. **Crea un proyecto.** Llama a `CreateProject` con el ID de tu tenant y un nombre. Los nombres son únicos dentro de un tenant, sin distinguir entre mayúsculas y minúsculas.
2. **Emite una clave.** Llama a `IssueApiKey` con el ID del proyecto y un permiso. La respuesta contiene el secreto una sola vez. Guárdalo en tu gestor de secretos.
3. **Registra usuarios.** Llama a `CreateEndUser` con la clave. Guarda el `id` devuelto junto a tu propio registro de usuario.
4. **Consulta usuarios.** Utiliza `GetEndUser` con el ID interno o `LookupEndUser` con el ID externo.
5. **Modifica o elimina usuarios.** Utiliza `UpdateEndUser` y `DeleteEndUser` con la `version` actual.

Todos los procedimientos están en `devbase.v1.DevBaseService`. Llama a DevBase desde código del servidor o desde la línea de comandos, con cualquier cliente de Connect o mediante JSON simple sobre HTTP. No lo llames desde un navegador ni pongas una clave de proyecto en el código del frontend: DevBase rechaza todos los orígenes de navegador excepto el de su propia consola. Establece `DEVBASE_URL` en la URL base de tu servidor de API de DevBase, por ejemplo, `http://127.0.0.1:46317` para un servidor local.

```sh
# Create a project with your Delino login token.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateProject" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"<tenant-id>","name":"My app"}'

# Issue a CRUD key for the project. The secret appears only in this response.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/IssueApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"<project-id>","permission":"API_KEY_PERMISSION_CRUD"}'

# Register an end user with the project key.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateEndUser" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DEVBASE_KEY" \
  --data '{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"<project-id>","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}'
```

En JSON, los nombres de los campos utilizan lowerCamelCase, los valores de enumeración utilizan sus nombres completos y los números de 64 bits, como `version`, son cadenas.

## Proyectos

- El nombre de un proyecto tiene entre 1 y 256 bytes, sin espacios al principio ni al final.
- `UpdateProject` cambia el nombre. `DeleteProject` elimina el proyecto, todos sus usuarios y todas sus claves en un solo paso.
- Un tenant puede tener hasta 100 proyectos.

## Claves de API del proyecto

- Un proyecto puede tener hasta 20 claves activas. Una clave permanece activa hasta que se revoca o caduca.
- Establece `expire_time` al emitir una clave si debe caducar. Si no lo haces, la clave no caduca automáticamente.
- DevBase solo almacena un hash del secreto. Si pierdes un secreto, revoca esa clave y emite una nueva. Para rotar una clave, emite primero la nueva y después revoca la antigua.
- Si se pierde la respuesta de `IssueApiKey`, repite la solicitud con la misma clave de idempotencia. La respuesta repetida contiene el ID y el permiso de la clave, pero no el secreto.
- `RevokeApiKey` surte efecto inmediatamente. Una clave revocada o caducada devuelve `unauthenticated`.

## Usuarios finales

| Campo         | Regla                                                                                                                                |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `external_id` | Obligatorio. De 1 a 256 bytes. Distingue entre mayúsculas y minúsculas. Único en el proyecto. Sin espacios al principio ni al final. |
| `name`        | Opcional. Hasta 256 bytes.                                                                                                           |
| `email`       | Opcional. Una dirección simple de hasta 254 bytes. Varios usuarios pueden tener el mismo correo electrónico.                         |
| `metadata`    | Un objeto JSON de hasta 16 KiB. Una solicitud completa puede tener hasta 64 KiB.                                                     |

- El mismo ID externo puede existir en distintos proyectos. Un proyecto puede tener hasta 1.000.000 de usuarios.
- La creación es explícita. DevBase no dispone de upsert ni de búsqueda por correo electrónico, nombre o metadatos.
- `UpdateEndUser` utiliza `update_mask` con `external_id`, `name`, `email` y `metadata`. Un `name` o `email` incluido en la máscara con un valor vacío lo borra. Un `metadata` incluido en la máscara reemplaza el objeto completo, y un objeto vacío lo borra. No puedes borrar el ID externo. Puedes cambiarlo y el ID interno permanece igual.
- `DeleteEndUser` elimina al usuario de forma inmediata y permanente. Puedes volver a utilizar el ID externo de inmediato. El siguiente usuario con ese ID recibe un nuevo ID interno y no tiene ningún vínculo con el usuario eliminado.
- DevBase no informa a otros servicios sobre las eliminaciones. Elimina los datos derivados en tu propia aplicación.

## Listas

`ListProjects`, `ListApiKeys` y `ListEndUsers` devuelven los elementos en orden de creación. El tamaño de página predeterminado es 50 y el máximo es 100. Repite la llamada con `next_page_token` hasta que esté vacío. Trata el token como opaco. Las listas no incluyen un recuento total. Si creas o eliminas elementos mientras lees las páginas, una página puede omitir o saltarse esos elementos. Un elemento que exista durante toda la lectura aparece una sola vez.

## Reintentos y conflictos

- Cada modificación necesita una `idempotency_key` de entre 8 y 128 caracteres de `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` y `-`. Un UUID funciona bien.
- Si repites una solicitud con la misma clave y el mismo contenido dentro de un plazo de 24 horas, obtienes el primer resultado y DevBase no vuelve a modificar nada. Utiliza esto para resolver respuestas perdidas. Después de 24 horas, la clave se considera una solicitud nueva.
- La misma clave con contenido diferente devuelve `already_exists`. Cada emisor de llamadas tiene sus propias claves, por lo que otro emisor no puede ver ni apropiarse de tu resultado.
- Una solicitud repetida vuelve a comprobarse con los permisos actuales. Tu acceso puede terminar entre la primera llamada y la repetición.
- Después de eliminar un usuario, la repetición de la creación o actualización de ese usuario devuelve `not_found` y DevBase no vuelve a crear al usuario. La repetición de la eliminación se completa correctamente. Después de eliminar un proyecto, las claves de ese proyecto dejan de funcionar.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject` y `DeleteEndUser` necesitan la `version` actual. Si alguien modificó primero el elemento, recibes `aborted`. Vuelve a leer el elemento y reintenta con la nueva versión.

## Límites

| Límite                                   | Valor predeterminado               |
| ---------------------------------------- | ---------------------------------- |
| Proyectos por tenant                     | 100                                |
| Usuarios por proyecto                    | 1.000.000                          |
| Claves activas por proyecto              | 20                                 |
| Solicitudes de usuarios por proyecto     | 1.000 por segundo, ráfaga de 2.000 |
| Solicitudes de administración por tenant | 50 por segundo, ráfaga de 100      |

Las solicitudes de usuarios se contabilizan conjuntamente para todas las claves y tokens de inicio de sesión de un proyecto. Las solicitudes de administración se contabilizan conjuntamente para un tenant. Una clave nueva no añade capacidad. Si superas un límite, recibes `resource_exhausted`. En el caso de un límite de frecuencia, el error y el encabezado `Retry-After` indican cuánto tiempo debes esperar. Un operador puede cambiar estos límites. Protegen el servicio y no constituyen una garantía de velocidad.

## Errores

| Código               | Significado                                                                                                           |
| -------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `unauthenticated`    | El token o la clave falta, no es válido, ha caducado o se ha revocado.                                                |
| `permission_denied`  | No tienes el rol o permiso de clave necesario, o el estado del tenant lo impide.                                      |
| `not_found`          | El elemento no existe, pertenece a otro proyecto o se eliminó.                                                        |
| `invalid_argument`   | Un valor de entrada no es válido.                                                                                     |
| `already_exists`     | Ya existe un nombre o ID externo, o se utilizó una clave de idempotencia con otro contenido.                          |
| `aborted`            | La versión es antigua.                                                                                                |
| `resource_exhausted` | Se aplica un límite de frecuencia o de recursos.                                                                      |
| `unavailable`        | Una dependencia o la base de datos falló. Es posible que una modificación se haya aplicado o que no se haya aplicado. |

Cada respuesta, y cada error que devuelve el propio DevBase, tiene un ID de seguimiento. Inclúyelo cuando solicites asistencia. Una solicitud de más de 256 KiB se rechaza antes, en la capa de transporte, con `resource_exhausted` y sin ID de seguimiento; en su lugar, reduce el tamaño de la solicitud. Si recibes `unavailable` en una modificación, repite la misma solicitud con la misma clave de idempotencia para averiguar si se aplicó.

## Estado del tenant

DevBase es gratuito. Tu plan, saldo y facturas nunca lo limitan. Solo pueden hacerlo el bloqueo por parte de un operador y la eliminación del tenant.

- Mientras un tenant esté bloqueado o se esté eliminando, las claves de API del proyecto no funcionan. Los Owners y Admins aún pueden leer proyectos y claves, revocar claves y eliminar usuarios y proyectos. No pueden crear ni modificar nada, emitir claves ni leer usuarios.
- Si DevBase no puede confirmar el estado del tenant, deniega la solicitud con `unavailable` o `permission_denied` y no hace suposiciones.
- Después de que un tenant se elimine permanentemente, DevBase elimina automáticamente sus proyectos, usuarios y claves.

## Lo que DevBase no hace

DevBase no inicia sesión por tus usuarios, no crea cuentas de Delino para ellos ni actúa en su nombre. No comparte usuarios entre proyectos, no mueve proyectos entre tenants, no importa ni exporta datos y no busca usuarios. Las copias de seguridad, si las hubiera, quedan fuera de la eliminación inmediata que DevBase aplica a sus datos activos.
