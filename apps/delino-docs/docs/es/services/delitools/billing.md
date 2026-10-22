# Facturación de DeliTools

DeliTools factura a tu tenant los tokens de IA que utilizan tus solicitudes. Delcore calcula el coste del uso y gestiona tu saldo, créditos y facturas. Esta página explica cómo el uso se convierte en un cargo.

## Qué se mide

- DeliTools lee los totales de **tokens de entrada** y **tokens de salida** que el proveedor de IA comunica para cada intento. Utiliza estos totales tal como se comunican.
- Los totales comunicados ya incluyen los tokens en caché o de razonamiento. DeliTools no vuelve a sumar esas partes, no ofrece un descuento independiente por la caché ni añade un recargo por el razonamiento.
- DeliTools comunica el uso de entrada y salida a Delcore por separado, y Delcore calcula su coste con sus respectivos precios.

## Precios

- El operador del servicio establece en Delcore un precio por cada dirección de los tokens (entrada y salida) para cada modelo. Los distintos modelos pueden tener precios diferentes.
- La traducción y la extracción comparten los mismos precios. La extracción siempre utiliza `openai/gpt-6-luna`, por lo que su cargo se calcula con los precios de entrada y salida de ese modelo.
- Un modelo que no tenga registrados ambos precios no puede ejecutarse. La solicitud falla con `ERROR_REASON_MODEL_UNAVAILABLE` y no se utiliza ningún otro modelo en su lugar.
- Un cambio de precio se aplica a las ejecuciones que comiencen después del cambio. El uso que ya haya comenzado conserva el precio vigente en su inicio.
- Tu plan determina el uso incluido, tu saldo y si puedes continuar una vez agotado el saldo. Consulta tu plan y tus facturas en Delcore.

## Qué se cobra

- **Se cobra cada intento cuyo uso se conoce.** Esto incluye un intento cuyo resultado no sea válido, cuya extracción falle, cuya respuesta haya llegado antes de que cancelaras o cuyo resultado nunca te llegue.
- **Los reintentos son intentos independientes.** Si DeliTools reintenta tras un fallo transitorio del proveedor y el intento fallido comunicó uso, se cobran ambos intentos. Un intento fallido que no haya comunicado uso no tiene coste.
- **No se cobra el uso desconocido.** Si el proveedor no comunica totales de tokens utilizables, DeliTools no estima ningún cargo ni consulta el uso posteriormente.
- **No se cobran las llamadas abortadas.** Si cancelas, o se agota el tiempo de espera de ejecución, mientras la llamada al proveedor sigue en curso, no llega ningún informe de uso y no se cobra nada, aunque el proveedor ya hubiera empezado a trabajar. Esta es una limitación conocida: DeliTools no consulta este uso posteriormente.
- **El uso debe registrarse.** DeliTools almacena el uso de cada intento antes de responder. Si no puede almacenarlo, la solicitud falla con `ERROR_REASON_DEPENDENCY_UNAVAILABLE` y no recibes ningún resultado.
- **No se cobran las solicitudes rechazadas.** Las solicitudes que no superan la validación, la autenticación o la autorización, o que alcanzan un límite o presentan un esquema no compatible, nunca llegan al proveedor de IA.
- **Cada llamada es nueva.** DeliTools no dispone de clave de idempotencia ni de reproducción de resultados. Si vuelves a enviar la misma solicitud, se ejecuta de nuevo y se cobra de nuevo.

## Cuándo aparecen los cargos

DeliTools guarda el uso de cada intento antes de responderte y después lo comunica a Delcore. La comunicación del uso es independiente de tu solicitud, por lo que un cargo puede aparecer en Delcore poco después de la llamada. Si Delcore no está disponible, DeliTools sigue intentándolo con los mismos identificadores, de modo que los reintentos no generan cargos duplicados.

Los registros de uso que DeliTools no pueda entregar se eliminan 30 días después de su creación, y los reintentos no prolongan ese plazo. El uso que no se haya entregado en ese plazo no se cobra. DeliTools no lo estima.

Puedes consultar tu uso y tus cargos en Delcore. Las respuestas de DeliTools no los incluyen.

## Comprobaciones de saldo y gasto excesivo

Delcore comprueba tu autorización y tu estado de facturación antes de cada solicitud y de nuevo antes de cada reintento. No reserva dinero ni tokens para la llamada. El coste solo se conoce una vez finalizada la llamada. Por tanto, varias solicitudes que se ejecuten al mismo tiempo pueden consumir más que tu saldo restante o tu uso incluido. Cuando se agota el saldo o se alcanza un límite, Delcore bloquea las solicitudes posteriores con `ERROR_REASON_BILLING_BLOCKED`.

Si no se puede contactar con Delcore, DeliTools rechaza las nuevas solicitudes de pago en lugar de ejecutarlas.

## Solicitudes bloqueadas

`ERROR_REASON_BILLING_BLOCKED` significa que Delcore ha bloqueado el uso de pago para tu tenant. Las causas habituales son un saldo agotado, una renovación fallida o un periodo de pago vencido, una disputa abierta, un bloqueo por parte del operador o un tenant que se está eliminando. Resuelve la causa en Delcore. Por ejemplo, recarga tu saldo o renueva tu suscripción. La gestión del tenant y la recuperación de pagos siguen disponibles mientras el uso de pago está bloqueado.
