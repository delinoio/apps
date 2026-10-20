# FixKey

FixKey corrige las palabras escritas con una distribución de teclado incorrecta. Admite Dubeolsik coreano y QWERTY de inglés estadounidense en ambas direcciones sin cambiar el idioma de entrada del sistema. FixKey se ejecuta localmente en tu equipo y no requiere una cuenta.

## Instalar FixKey

Abre [Versiones de FixKey](https://github.com/delinoio/apps/releases?q=fixkey%40) y elige el paquete correspondiente a tu sistema operativo y procesador:

- macOS ARM64: DMG para Apple Silicon. Mueve FixKey a Aplicaciones y ábrelo.
- Windows x64 o ARM64: instalador EXE para tu procesador. Los paquetes de Windows no tienen una firma de editor de Authenticode, por lo que Windows puede mostrar una advertencia de editor desconocido o de SmartScreen.
- Linux X11 x64 o ARM64: AppImage para tu procesador. Wayland no es compatible.

Los requisitos mínimos de ejecución son macOS 11 en Apple Silicon; Windows 10 con WebView2; o Linux con X11, GTK 3, WebKitGTK 4.1 y un bus de accesibilidad AT-SPI. Estos son requisitos previos de la plataforma. No garantizan la compatibilidad con todas las aplicaciones o campos de texto.

## Conceder permisos

FixKey necesita los permisos de accesibilidad y entrada de tu sistema operativo para leer y sustituir la palabra actual en un campo editable. Sigue el cuadro de diálogo de permisos, concede el acceso solicitado en los ajustes del sistema y, después, selecciona **Volver a comprobar**. La corrección permanece inactiva hasta que los permisos necesarios y los diccionarios incluidos estén listos. Se excluyen los campos de contraseña y los campos protegidos.

En macOS, comprueba **Accesibilidad** y **Monitorización de entrada** en Ajustes del Sistema. Accesibilidad puede conceder acceso efectivo a la entrada sin una entrada independiente en Monitorización de entrada. Para comprobar esa lista, abre **General** en FixKey y selecciona **Configurar la monitorización de entrada**. Si FixKey no aparece, usa **+** en la lista de permisos del sistema para añadir la aplicación FixKey.app actual desde Aplicaciones; después, vuelve a FixKey y realiza otra comprobación. No se requiere una entrada independiente en la lista cuando macOS ya concede acceso efectivo a la entrada.

## Corregir una palabra

Ambas distribuciones están activadas de forma predeterminada. Escribe en un campo editable y termina una palabra con un espacio o un signo de puntuación. FixKey compara los candidatos de las distribuciones activadas con sus diccionarios incluidos. Solo sustituye la palabra cuando exactamente uno de los idiomas activados reconoce su candidato y este difiere del original. Si ningún idioma reconoce un candidato, o si lo reconoce más de uno, la palabra permanece sin cambios.

Por ejemplo, las secuencias de teclas `rPthrgo` y `zlqhemrk` pueden convertirse en `계속해` y `키보드가`. Estos ejemplos ilustran la corrección de distribución; los resultados dependen del campo actual, el contexto de entrada y el reconocimiento del diccionario.

FixKey conserva el separador que aparece después de una palabra corregida. Intro y Tabulador no activan la corrección. Los signos de puntuación utilizados en URL o direcciones de correo electrónico pueden retrasar la evaluación hasta que FixKey pueda clasificar el token completo. Los apóstrofos y guiones situados entre letras permanecen dentro de la palabra.

La corrección automática omite el texto pegado o insertado mediante programación, los números, los emojis, las URL, las direcciones de correo electrónico y los tokens que combinan sistemas de escritura. Espera a que termine la composición de entrada en coreano. Los campos desconocidos, los permisos no disponibles y los campos que no pueden modificarse de forma segura permanecen intactos.

### Deshacer una corrección

Pulsa Retroceso inmediatamente después de una corrección automática para restaurar la palabra original y su separador. Las pulsaciones posteriores de Retroceso eliminan texto normalmente. Mover el cursor, cambiar la selección, cambiar de campo o empezar otra palabra finaliza este contexto de deshacer inmediato.

### Elegir manualmente un candidato

1. Abre **Atajos** y asigna el atajo para candidatos manuales. No está asignado de forma predeterminada. Si el registro falla, comprueba el motivo mostrado o elige otro atajo.
2. Escribe una palabra en un campo editable y utiliza el atajo.
3. Usa las teclas de flecha para seleccionar un candidato. Pulsa Intro para aplicarlo o Esc para cerrar el selector.

El selector muestra los resultados del reconocimiento del diccionario. Puedes elegir explícitamente un candidato no reconocido. Mover el cursor del campo de origen o cambiar de aplicación cierra el selector. El selector no está disponible cuando FixKey no puede establecer un destino de sustitución seguro.

## Configurar la corrección

En **General**, puedes pausar o reanudar la corrección, seleccionar los idiomas activados, habilitar el inicio al iniciar sesión y elegir el idioma y el tema de la interfaz. El inicio al iniciar sesión está desactivado de forma predeterminada. Los ajustes se aplican inmediatamente y se guardan localmente.

Para mantener una aplicación sin cambios, añádela en **Aplicaciones excluidas**. Las exclusiones identifican la aplicación en lugar de limitarse a su nombre mostrado. Las aplicaciones que no se puedan resolver permanecen en la lista para que puedas revisarlas.

## Privacidad y uso sin conexión

FixKey procesa la entrada en tu dispositivo. Nunca guarda ni transmite el texto introducido, los candidatos, el contenido de los documentos, los títulos de las ventanas ni el historial de correcciones. No utiliza el portapapeles para realizar sustituciones. No hay telemetría, informes remotos de fallos ni carga automática de diagnósticos.

Los diccionarios están incluidos, por lo que la corrección funciona sin conexión desde el primer inicio una vez que los permisos están listos. Los diccionarios cambian con las actualizaciones de la aplicación; no hay una descarga independiente de diccionarios ni un editor de diccionarios de usuario. El uso de la red se limita a la comprobación y descarga de actualizaciones.

## Actualizar FixKey

FixKey comprueba si hay actualizaciones estables al iniciarse, cada 24 horas mientras está en ejecución y cuando solicitas una comprobación en **Actualizaciones**. La descarga y la instalación requieren tu consentimiento. Se verifican las firmas de los paquetes de actualización y, si una firma no es válida, se impide la instalación. La corrección se detiene antes de la instalación y los ajustes se conservan.

Después de una comprobación fallida, vuelve a intentarlo manualmente o espera a la siguiente comprobación programada. Si la instalación falla, reinicia FixKey o vuelve a instalar el paquete adecuado desde Versiones de FixKey. Puedes reinstalar manualmente una versión anterior; FixKey nunca vuelve automáticamente a una versión anterior.

## Solución de problemas y asistencia

Si la corrección está inactiva, abre **Diagnóstico y ayuda** y consulta el estado y el código de motivo. Vuelve a comprobar los permisos y prueba un campo editable en otra aplicación. Un estado de espera de un campo editable significa que el campo actual no se pudo identificar de forma segura. Comprueba también si la corrección está en pausa o si la aplicación está excluida.

Usa **Abrir carpeta de registros** para inspeccionar los registros de diagnóstico locales o **Eliminar registros** para borrarlos. Los registros contienen resultados de diagnóstico y códigos de motivo, no el contenido de la entrada. FixKey conserva como máximo siete días de registros y un total máximo de 10 MiB.

Para obtener ayuda, utiliza el enlace a GitHub Issues en **Diagnóstico y ayuda** o abre [GitHub Issues](https://github.com/delinoio/apps/issues). Incluye la versión de FixKey, el sistema operativo, la aplicación afectada y el código de motivo. No incluyas texto privado ni credenciales.
