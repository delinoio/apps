# Claves de API de DeliTools

Las claves de API permiten que tus servidores llamen a DeliTools sin un token de inicio de sesión de Delino. Delcore crea y verifica las claves para todos los servicios de Delino, por lo que el mismo sistema de claves también funciona para otros servicios. Administras las claves con tu token de inicio de sesión de Delino y usas una clave como token de portador al llamar a un servicio.

## Tipos de claves

| Tipo                         | Actúa como                                         | Quién puede crearla, enumerarla y revocarla                    | Permisos al usarla                                                                                    |
| ---------------------------- | -------------------------------------------------- | -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Clave personal               | Tú, dentro de un único tenant                      | Solo tú, y únicamente mientras seas miembro actual del tenant. | Las autorizaciones de la clave y tus permisos actuales en el tenant. Ambos deben permitir la llamada. |
| Clave de servicio del tenant | El tenant, independientemente de cualquier usuario | Un propietario o administrador actual del tenant.              | Solo las autorizaciones de la clave. Sigue funcionando si la persona que la creó abandona el tenant.  |

- Los demás usuarios nunca pueden ver ni revocar tus claves personales, incluidos los propietarios y administradores. Los roles de facturación y miembro no pueden administrar claves de servicio del tenant.
- Una identidad de máquina a máquina no puede administrar claves. Las claves nunca funcionan como credenciales para el propio Delcore, sino solo para los servicios que autorizan.
- Ambos tipos están vinculados a un único tenant. Una clave no se puede usar para otro tenant.

## Crear una clave

Llama a `delcore.v1.DelcoreService/CreateApiKey` en `https://delcore.api.delino.io` con tu token de inicio de sesión de Delino.

| Campo        | Reglas                                                                                                                                                                     |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | Un UUID v7 que generas tú. Repetir la solicitud con la misma clave y los mismos datos de entrada devuelve el mismo resultado.                                              |
| `tenantId`   | El tenant al que pertenece la clave.                                                                                                                                       |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` o `API_KEY_OWNER_KIND_TENANT`.                                                                                                               |
| `name`       | De 1 a 100 bytes, no puede estar en blanco ni contener caracteres de control.                                                                                              |
| `grants`     | De 1 a 16 autorizaciones. Cada una tiene un `serviceId` y de 1 a 32 `capabilities` de hasta 100 bytes. Cada servicio aparece una sola vez y las capacidades no se repiten. |
| `expiresAt`  | Fecha y hora opcionales en formato RFC 3339, en el futuro. Si no se especifica, la clave es válida hasta que la revoques.                                                  |

Busca el `serviceId` y las capacidades de un servicio en el catálogo de Delcore (`GetCatalog`). Para DeliTools, concede las capacidades `translate` y `extract` que necesites. Delcore solo acepta capacidades que el servicio haya declarado, por lo que una capacidad mal escrita se rechaza al crear la clave.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}],"expiresAt":"2027-01-01T00:00:00Z"}'
```

La respuesta contiene los detalles de la clave y el `secret`, que comienza por `dlk_`.

- **El secreto aparece una sola vez.** Cópialo ahora en tu gestor de secretos. Delcore solo conserva un hash y nunca muestra el secreto en las listas ni en una respuesta repetida.
- **Secreto perdido.** Si pierdes el secreto, o si la respuesta a una solicitud repetida tiene un `secret` vacío, crea una clave nueva y revoca la anterior.
- **Trátalo como una contraseña.** Mantenlo en el código del servidor y fuera de los registros, los repositorios y los paquetes del frontend.
- Cada tenant puede tener un número limitado de claves activas: 20 claves personales por usuario y 20 claves de tenant en total. Las claves revocadas y caducadas no cuentan.

Las autorizaciones y la fecha de caducidad quedan fijadas al crear una clave. No hay ninguna llamada para editar, renovar o rotar claves. Para cambiar una clave, crea una sustituta con las nuevas autorizaciones o la nueva fecha de caducidad, configura tu aplicación para que la use y, después, revoca la clave anterior.

## Enumerar y revocar claves

- `ListApiKeys` recibe `tenantId` y `ownerKind`. Devuelve los detalles de las claves, incluidas las revocadas y caducadas, pero nunca los secretos. Las listas de claves personales solo muestran tus propias claves. Las listas de claves del tenant requieren el rol de propietario o administrador. Usa `page.afterId` con el `nextAfterId` devuelto para leer más resultados.
- `RevokeApiKey` recibe un `requestKey`, un `tenantId` y un `keyId`. Surte efecto de inmediato para las nuevas solicitudes. Volver a revocar una clave es seguro.

## Usar una clave

Envía el secreto como `Authorization: Bearer dlk_...` al servicio que autoriza la clave. En DeliTools, esto funciona para `Translate` y `Extract`, tal como se describe en la [referencia de la API](/es/services/delitools/api-reference).

- `tenantId` es opcional al usar una clave. Si lo envías, debe ser el tenant de la clave.
- Una clave solo puede usar las capacidades que hayas concedido para ese servicio. Una clave para un servicio nunca funciona para otro.
- Delcore comprueba la clave en cada solicitud y de nuevo antes de cada reintento de IA. La revocación, la caducidad y la pérdida de permisos o de acceso a la facturación impiden nuevas solicitudes y reintentos posteriores. Una llamada a un proveedor que ya esté en curso puede finalizar.
- Una clave personal deja de funcionar mientras ya no tengas permiso para usar la capacidad en el tenant, por ejemplo, después de que te eliminen del tenant o pierdas un permiso de grupo. Eliminarte no elimina la clave.
- Una clave de tenant no depende de que su creador siga siendo miembro.
- Las llamadas con una clave se facturan al tenant de la clave a los mismos precios que las llamadas con un token de inicio de sesión. Consulta [Facturación](/es/services/delitools/billing).
- Cuando eliminas un tenant, Delcore elimina sus claves cuando la eliminación se hace definitiva. Las claves personales se eliminan en cuanto se acepta la baja del usuario. Una clave de un tenant eliminado no es válida.

## Errores comunes

| Situación                                                                                    | Resultado                                                           |
| -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| El secreto está mal escrito, es desconocido, está revocado o ha caducado                     | `unauthenticated`. Delcore no indica cuál de estos casos se aplica. |
| La clave carece del servicio o de la capacidad                                               | `permission_denied`                                                 |
| `tenantId` no coincide con el tenant de la clave                                             | `permission_denied`                                                 |
| Tu rol no puede administrar este tipo de clave                                               | `permission_denied` de Delcore al crearla, enumerarla o revocarla.  |
| Revocas la clave personal de otro usuario o una clave desconocida                            | `not_found` de Delcore.                                             |
| Creas una clave en un tenant que se está eliminando o superas el límite de claves            | `failed_precondition` de Delcore.                                   |
| La autorización, el nombre, el servicio, la capacidad o la fecha de caducidad no son válidos | `invalid_argument` de Delcore al crear la clave.                    |
| La facturación del tenant está bloqueada                                                     | DeliTools devuelve `ERROR_REASON_BILLING_BLOCKED`.                  |
