# Git Tracker

Realiza el seguimiento de mediciones numéricas para commits de Git en [git-tracker.delino.io](https://git-tracker.delino.io). Git Tracker es gratuito. Admite repositorios de GitHub públicos y privados, incluidas las solicitudes de incorporación de cambios de forks. Úsalo para tamaños de archivo, tiempos, rendimiento, recuentos y otras mediciones escalares.

## Conectar un repositorio

1. Inicia sesión con tu cuenta de Delino y selecciona tu espacio personal o de organización.
2. Pide a un propietario del espacio que te conceda permisos de visualización, edición o administración de Git Tracker mediante los grupos del espacio. La visualización permite realizar lecturas. La edición añade cargas y finalización. La administración añade conexión, gestión de tokens y eliminación permanente.
3. Instala la GitHub App de Git Tracker para el repositorio.
4. Abre **Administrar repositorios**. Introduce `owner/repository` y un token de acceso de GitHub para tu cuenta de GitHub vinculada que tenga acceso de administrador al repositorio. El servicio verifica tu autoridad en GitHub y no conserva ese token de acceso.
5. Conecta el repositorio. También necesitas autoridad de propietario/administrador del espacio de Delino. Un repositorio solo puede estar conectado a un espacio a la vez. Copia su ID de conexión desde la administración del repositorio para las llamadas de la Action y de la API. Después de desconectarlo, otro espacio autorizado puede conectarlo con un nuevo ID de conexión; el historial conservado del espacio anterior permanece privado.

Las mediciones de repositorios públicos pueden leerse sin iniciar sesión. Las mediciones privadas requieren pertenencia al espacio y permiso de lectura de Git Tracker. El acceso al espacio se aplica a todos los repositorios conectados en ese espacio.

Desconectar o eliminar la App detiene las cargas y hace que el historial conservado sea privado. Los miembros autorizados aún pueden leerlo. Un cambio confirmado de público a privado hace que los datos históricos sean privados. Si GitHub no puede confirmar la visibilidad actual, Git Tracker conserva la última visibilidad confirmada hasta que la sincronización se complete correctamente.

## Definir tu conjunto de datos

Mantén una configuración YAML en tu repositorio. Los ID son estables; los nombres pueden cambiar sin perder el historial. Todas las columnas de una fila deben cargarse juntas.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

Carga filas JSON, por ejemplo, una tabla de tamaños de archivo al estilo de SWC:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

La Action conserva la representación decimal de JSON sin convertirla a coma flotante binaria. Los clientes de Direct Connect envían los valores decimales como cadenas, por ejemplo `"9007199254740993.0000000000000001"`. Se admiten números negativos y decimales finitos. El redondeo de visualización nunca cambia las decisiones sobre regresiones.

Las unidades admitidas son `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; y `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Las unidades compatibles se convierten antes de la comparación. Los tipos incompatibles no son comparables. Por ejemplo, 1048576 B se muestra como 1 MiB; los valores exactos permanecen visibles en las tablas.

Las direcciones son `lower`, `higher` y `neutral`. Las columnas neutrales nunca producen regresiones. Los umbrales se aplican a todas las filas de la columna. Si existen umbrales absolutos y porcentuales, superar cualquiera de ellos constituye una regresión; la igualdad no. Sin un umbral, los cambios se muestran, pero no provocan fallos por regresión. Solo se evalúan los cambios en la dirección desfavorable.

El cambio porcentual es `(new - baseline) / abs(baseline) * 100`. Una línea base de cero no tiene cambio porcentual. Un umbral absoluto aún puede determinar el resultado; una regla exclusivamente porcentual aplaza la evaluación. Las comparaciones usan la configuración almacenada de la ejecución de destino, no una configuración posterior.

## Usar la GitHub Action oficial

Fija `delinoio/apps/actions/git-tracker` a un SHA completo de commit que haya sido revisado. Sustituye los marcadores de posición siguientes por tu ID de conexión de Git Tracker, la revisión del checkout y la revisión de la Action. Concede `id-token: write` para OIDC. Un token de carga del repositorio es una alternativa; almacénalo como secreto de GitHub y pásalo mediante `token`.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

Los trabajos independientes comparten el ID de ejecución predeterminado, `repository_id/run_id/run_attempt`, y pueden cargar filas que no se solapen. Usa un nuevo ID de ejecución para las reejecuciones. OIDC de Actions requiere la identidad predeterminada; los ID de ejecución personalizados son para clientes que usan tokens de repositorio. Las cargas OIDC desde forks están limitadas al commit principal de la PR y no pueden proporcionar anulaciones de la línea base ni escribir en el historial de pushes del repositorio de origen.

Cada trabajo de carga evalúa únicamente sus propias filas cargadas. Las regresiones se notifican sin hacer que CI falle de forma predeterminada. Establece `fail-on-regression: 'true'` para que ese trabajo falle cuando haya regresiones. Las comparaciones ausentes no hacen que CI falle. El trabajo de finalización solo indica que la medición ha terminado; no envía el estado de éxito, fallo o cancelación de CI ni realiza una validación agregada. Una finalización vacía es válida cuando se omitieron todos los trabajos.

Los errores de red y los fallos transitorios de la API se reintentan como máximo tres veces. De forma predeterminada, el trabajo falla después de los reintentos. Establece `warn-on-api-error: 'true'` para mostrar una advertencia y continuar correctamente. Los fallos de los comentarios de PR solo generan una advertencia; no invalidan los datos guardados ni hacen que CI falle por sí solos. La Action registra un enlace con detalles y escribe un resumen del trabajo.

Un comentario de PR refleja los resultados acumulados de la última ejecución registrada en el commit más reciente de la PR. Las ejecuciones anteriores siguen disponibles en el historial web.

## Llamar a la API de Connect

El origen de la API es `https://git-tracker.api.delino.io`. Las solicitudes JSON de Connect usan `Content-Type: application/json` y `Connect-Protocol-Version: 1`. Los administradores pueden emitir tokens de repositorio con una fecha de caducidad futura obligatoria y revocarlos desde la administración del repositorio.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` contiene `context`, `dataset` y `rows`:

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

Llama a `/gittracker.v1.GitTrackerService/Complete` con el mismo `context`. Para las cargas de PR, incluye `pullRequest` y usa la rama base y el commit principal de la PR. `baselineCommit` selecciona explícitamente un SHA completo como línea base. Los commits históricos usan la misma API de carga. No conviertas las cadenas decimales mediante `Number` de JavaScript antes de enviarlas.

## Reglas de ejecución y comparación

La primera carga fija la configuración de cada conjunto de datos para esa ejecución. Una configuración posterior diferente se rechaza. Repetir una fila existente con valores numéricos idénticos es idempotente. Los valores en conflicto se rechazan. Un lote es atómico: si una fila no es válida o entra en conflicto, no se guarda ninguna parte de esa solicitud.

Los datos aparecen inmediatamente como **Midiendo**. Solo pasan a ser representativos después de la finalización. El resultado representativo de un commit es su última ejecución registrada que se haya completado, incluidas las ejecuciones con regresiones. Una ejecución anterior que termine tarde no sustituye a una ejecución registrada más reciente que ya se haya completado. Repetir la finalización no cambia ni la hora de finalización ni el orden.

Después de seis horas sin que se guarden nuevos datos de medición, una ejecución sin finalizar pasa a estar **Interrumpida**. Las retransmisiones idénticas no reinician ese temporizador. Las ejecuciones completadas e interrumpidas rechazan cargas adicionales; las ejecuciones interrumpidas también rechazan la finalización. Inicia una ejecución nueva en su lugar. Los datos se conservan hasta que los elimines.

Las comparaciones automáticas de PR fijan el commit de base de fusión y su ejecución representativa completada en la primera carga. Una línea base ausente sigue sin estar disponible aunque se cargue posteriormente. Las comparaciones automáticas de pushes recorren por separado la ascendencia del primer padre para cada fila y usan el antepasado medido más cercano que se hubiera completado antes de la hora límite de la primera carga. Los trabajos posteriores usan esa misma hora límite. La tabla identifica cada commit y ejecución de origen reales. Las comparaciones con un SHA de línea base explícito no usan la búsqueda alternativa de antepasados por fila.

La comparación de dos commits del sitio web usa únicamente las ejecuciones representativas completadas de los commits seleccionados. Las filas y columnas nuevas y los datos ausentes o incompatibles no son comparables y no provocan fallos por regresión. Los datos de línea base eliminados no se reconstruyen. Antes de la finalización, una fila ausente no se ha recopilado; después de la finalización, se considera que falta.

Los gráficos usan commits espaciados uniformemente según el orden del primer padre. Conectan los puntos medidos reales a través de los commits omitidos y nunca trasladan ni inventan valores. Filtra por conjunto de datos, rama, columna, elementos, fechas e intervalo de commits. Usa la tabla de datos para ver los valores exactos y los enlaces de ejecución accesibles mediante teclado.

## Eliminación y solución de problemas

Los administradores pueden eliminar permanentemente las mediciones de un repositorio, un conjunto de datos o una ejecución después de confirmarlo. Una comparación cuya línea base se haya eliminado deja de estar disponible. Cuando Delcore finaliza la eliminación del espacio, Git Tracker elimina las conexiones, los tokens, la configuración y el historial del espacio.

Si se produce un error de permisos, verifica el espacio seleccionado y tu rol actual de Git Tracker. En caso de conflictos de carga, confirma la identidad de la ejecución, la configuración del conjunto de datos y los valores completos de las filas. Para las ejecuciones interrumpidas o completadas, usa un nuevo ID de ejecución. En caso de errores de red o del servidor, vuelve a intentarlo y conserva el ID de correlación de la respuesta cuando solicites ayuda. Nunca compartas tokens, credenciales ni mediciones privadas en informes públicos.

Las mediciones de ejecuciones de PR siguen siendo consultables, pero no proporcionan puntos para el gráfico de ramas del repositorio de origen ni líneas base automáticas. Esto impide que las mediciones de forks sustituyan los datos o la configuración de la rama de línea base para un SHA de commit compartido.
