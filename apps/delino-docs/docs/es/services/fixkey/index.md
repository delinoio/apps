# FixKey

FixKey corrige las palabras escritas con una distribución de teclado incorrecta. Es compatible con Dubeolsik coreano y QWERTY de inglés estadounidense en ambas direcciones. FixKey se ejecuta localmente en tu equipo y no requiere una cuenta.

## Instalar FixKey

Abre [las versiones de FixKey](https://github.com/delinoio/apps/releases?q=fixkey%40) y elige el paquete correspondiente a tu sistema operativo y procesador:

- macOS ARM64: DMG para Apple silicon. Mueve FixKey a Aplicaciones y ábrelo.
- Windows x64 o ARM64: instalador EXE para tu procesador. Los paquetes de Windows no tienen una firma de editor de Authenticode, por lo que Windows puede mostrar una advertencia de editor desconocido o de SmartScreen.
- Linux X11 x64 o ARM64: AppImage para tu procesador. Wayland no es compatible.

Los requisitos mínimos de ejecución son macOS 11 en Apple silicon; Windows 10 con WebView2; o Linux con X11, GTK 3, WebKitGTK 4.1 y un bus de accesibilidad AT-SPI. Estos son requisitos previos de la plataforma. No garantizan la compatibilidad con todas las aplicaciones o campos de texto.

## Conceder permisos

FixKey necesita los permisos de accesibilidad y entrada de tu sistema operativo para leer y reemplazar la palabra actual en un campo editable. Sigue el cuadro de diálogo de permisos, concede el acceso solicitado en los ajustes del sistema y, a continuación, elige **Volver a comprobar**. La corrección permanece inactiva hasta que estén listos los permisos necesarios y los diccionarios incluidos. Los campos de contraseña y protegidos quedan excluidos.

En macOS, comprueba **Accesibilidad** y **Monitorización de entrada** en Ajustes del Sistema. Accesibilidad puede conceder acceso efectivo a la entrada sin una entrada independiente en Monitorización de entrada. Para comprobar esa lista, abre **General** en FixKey y elige **Configurar Monitorización de entrada**. Si FixKey no aparece, usa **+** en la lista de permisos del sistema para añadir la aplicación FixKey.app actual desde Aplicaciones; después, vuelve a FixKey y realiza una nueva comprobación. No se requiere una entrada independiente en la lista cuando macOS ya concede acceso efectivo a la entrada.

## Corregir una palabra

Ambas distribuciones están habilitadas de forma predeterminada. Escribe en un campo editable y termina una palabra con un espacio o un signo de puntuación. FixKey contrasta los candidatos de las distribuciones habilitadas con sus diccionarios incluidos. Solo reemplaza la palabra cuando exactamente un idioma habilitado reconoce su candidato y dicho candidato difiere del original. Si ningún idioma o más de uno reconoce un candidato, la palabra no se modifica.

Por ejemplo, las secuencias de teclas `rPthrgo` y `zlqhemrk` pueden convertirse en `계속해` y `키보드가`. Estos ejemplos ilustran la corrección de distribución; los resultados dependen del campo actual, el contexto de entrada y el reconocimiento del diccionario.

FixKey conserva el separador situado después de una palabra corregida. Intro y Tab no activan la corrección. La puntuación utilizada en direcciones URL o de correo electrónico puede retrasar la evaluación hasta que FixKey pueda clasificar el token completo. Los apóstrofos y guiones entre letras permanecen dentro de la palabra.

La corrección automática omite el texto pegado o insertado mediante programación, los números, los emojis, las direcciones URL, las direcciones de correo electrónico y los tokens con escrituras mixtas. Espera a que termine la composición de entrada en coreano. Los campos desconocidos, los permisos no disponibles y los campos que no puedan modificarse de forma segura permanecen intactos.

Tras una corrección automática correcta en macOS o Windows, FixKey también intenta seleccionar una fuente de entrada habilitada para el idioma corregido. En Windows, puede utilizar los modos hangul y latino del IME coreano de Microsoft, incluso cuando no está habilitada una fuente QWERTY estadounidense independiente. Mantén tu IME coreano configurado para Dubeolsik. Si la selección de la fuente de entrada no está disponible o falla, el texto corregido y la opción de deshacer inmediatamente siguen estando disponibles. Los IME de Windows cerrados o no compatibles no se modifican. Linux mantiene la fuente de entrada actual. La acción inmediata de deshacer no vuelve a cambiar la fuente de entrada.

### Deshacer una corrección

Pulsa Retroceso inmediatamente después de una corrección automática para restaurar la palabra original y su separador. Las pulsaciones posteriores de Retroceso eliminan caracteres con normalidad. Mover el cursor, cambiar la selección, cambiar de campo o empezar otra palabra finaliza este contexto inmediato para deshacer.

### Elegir un candidato manualmente

1. Abre **Atajos** y asigna el atajo para candidatos manuales. No tiene ninguna asignación de forma predeterminada. Si el registro falla, consulta el motivo mostrado o elige otro atajo.
2. Escribe una palabra en un campo editable y usa el atajo.
3. Usa las teclas de flecha para seleccionar un candidato. Pulsa Intro para aplicarlo o Esc para cerrar el selector.

El selector muestra los resultados de reconocimiento del diccionario. Puedes elegir explícitamente un candidato no reconocido. Mover el cursor de origen o cambiar de aplicación cierra el selector. El selector no está disponible cuando FixKey no puede establecer un destino de reemplazo seguro.

## Configurar la corrección

En **General**, puedes pausar o reanudar la corrección, seleccionar los idiomas habilitados, activar el inicio de sesión y elegir el idioma y el tema de la interfaz. El inicio de sesión está desactivado de forma predeterminada. Los ajustes se aplican inmediatamente y se guardan localmente.

Para evitar cambios en una aplicación, añádela en **Aplicaciones excluidas**. Las exclusiones identifican la aplicación, no solo su nombre mostrado. Las aplicaciones no resueltas permanecen en la lista para que puedas revisarlas.

## Privacidad y uso sin conexión

FixKey procesa la entrada en tu dispositivo. Nunca guarda ni transmite el texto introducido, los candidatos, el contenido de los documentos, los títulos de las ventanas ni el historial de correcciones. No utiliza el portapapeles para realizar reemplazos. No hay telemetría, informes remotos de fallos ni carga automática de diagnósticos.

Los diccionarios están incluidos, por lo que la corrección funciona sin conexión desde el primer inicio una vez que los permisos están listos. Los diccionarios cambian con las actualizaciones de la aplicación; no hay ninguna descarga de diccionarios independiente ni un editor de diccionarios de usuario. El uso de la red se limita a la comprobación y descarga de actualizaciones.

## Actualizar FixKey

FixKey comprueba si hay actualizaciones estables al iniciarse, cada 24 horas mientras está en ejecución y cuando solicitas una comprobación en **Actualizaciones**. La descarga y la instalación requieren tu consentimiento. Las firmas de las cargas de actualización se verifican, y las firmas no válidas impiden la instalación. La corrección se detiene antes de la instalación y los ajustes se conservan.

Después de una comprobación fallida, vuelve a intentarlo manualmente o espera a la siguiente comprobación programada. Si la instalación falla, reinicia FixKey o reinstala el paquete adecuado desde las versiones de FixKey. Puedes reinstalar manualmente una versión anterior; FixKey nunca revierte automáticamente a una versión anterior.

## Solución de problemas y asistencia

Si la corrección está inactiva, abre **Diagnóstico y ayuda** y consulta el estado y el código del motivo. Vuelve a comprobar los permisos y prueba un campo editable en otra aplicación. Un estado de espera de un campo editable significa que no se pudo identificar de forma segura el campo actual. Comprueba también si la corrección está en pausa o si la aplicación está excluida.

Usa **Abrir carpeta de registros** para inspeccionar los registros de diagnóstico locales o **Eliminar registros** para borrarlos. Los registros contienen resultados de diagnóstico y códigos de motivo, no el contenido introducido. FixKey conserva como máximo siete días de registros y un máximo total de 10 MiB.

Para obtener ayuda, usa el enlace de GitHub Issues en **Diagnóstico y ayuda** o abre [GitHub Issues](https://github.com/delinoio/apps/issues). Incluye la versión de FixKey, el sistema operativo, la aplicación afectada y el código del motivo. No incluyas texto privado ni credenciales.
