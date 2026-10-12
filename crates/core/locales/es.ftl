# Nightpost, español. Traducción por revisar por un hablante nativo.
# Un mensaje por línea; el programa rellena { $name }.

## Fechas

today = Hoy
yesterday = Ayer
date-this-year = { $weekday } { $day } de { $month }
date-other-year = { $day } de { $month } de { $year }
date-short = { $day } { $month }
weekday-1 = lunes
weekday-2 = martes
weekday-3 = miércoles
weekday-4 = jueves
weekday-5 = viernes
weekday-6 = sábado
weekday-7 = domingo
month-1 = enero
month-2 = febrero
month-3 = marzo
month-4 = abril
month-5 = mayo
month-6 = junio
month-7 = julio
month-8 = agosto
month-9 = septiembre
month-10 = octubre
month-11 = noviembre
month-12 = diciembre
month-short-1 = ene
month-short-2 = feb
month-short-3 = mar
month-short-4 = abr
month-short-5 = may
month-short-6 = jun
month-short-7 = jul
month-short-8 = ago
month-short-9 = sept
month-short-10 = oct
month-short-11 = nov
month-short-12 = dic

## Tor, según el daemon ("Tor: " se queda tal cual: la interfaz lo busca)

tor-percent = Tor: { $percent } %
tor-stuck = Tor: ¿atascado? { $why }
tor-error = Tor: { $error }
tor-switch-failed = Tor: el cambio falló: { $error }
tor-reconnecting = Tor: reconectando…
tor-reconnecting-bridges = Tor: reconectando a través de los puentes…
tor-to-bridge = Tor: ¿bloqueado en esta red? Pasando por un puente…
tor-from-bridge = Tor: el puente no pasa; probando sin él…

## Las respuestas del daemon

err-starting = Tor aún no está listo: inténtalo de nuevo en un momento.
err-already-contact = Ya es un contacto.
err-name-taken = { $name } ya es un contacto.
err-code-waiting = Ya hay un código esperando para { $name }.
err-not-confirmed = No confirmado.
err-did-not-confirm = { $name } no confirmó.
err-nobody-waiting = Nadie espera con ese nombre.
err-code-length = Un código tiene 12 cifras.
err-code-typo = Este código tiene una errata: una cifra está mal o dos están intercambiadas.
err-not-found = No se encontró en 5 minutos: ¿tiene Nightpost abierto?
err-no-contact = Ese contacto no existe.
err-invite-not = Esto no es una invitación de Nightpost.
err-invite-version = Esta invitación viene de otra versión de Nightpost: uno de los dos tiene que actualizar.
err-invite-malformed = Esta invitación está incompleta o mal copiada.
err-invite-signature = Esta invitación ha sido alterada.
err-invite-expired = Esta invitación ha caducado: pide una nueva.
err-passphrase-keychain = Sin llavero, la frase de contraseña no se puede cambiar aquí.
err-passphrase-empty = Una frase de contraseña vacía no protege nada.
err-passphrase-wrong = Esa no es la frase de contraseña actual.
err-unexpected = Respuesta inesperada: { $reply }

## Inicio

connecting-daemon = Conectando con el daemon…
starting = Iniciando…
restarting-update = Reiniciando Nightpost tras la actualización…
daemon-stopped = El daemon se ha detenido. Vuelve a abrir Nightpost para iniciarlo.
daemon-silent = El daemon de Nightpost no responde: { $error }
bridges-button = ¿Tor bloqueado aquí? Puentes
unlock-prompt = Tu frase de contraseña abre tus mensajes.
unlock = Desbloquear
wrong-passphrase = Esa frase de contraseña no los abre. Inténtalo de nuevo.

## Ventana

minimise = Minimizar
maximise = Maximizar
restore = Restaurar
close = Cerrar
show-list = Mostrar la lista
hide-list = Ocultar la lista

## La lista de personas

reachable-count = { $reachable } de { $total } disponibles
preview-you = Tú: { $text }
no-messages = Aún no hay mensajes
add-someone = Añadir a alguien
settings = Ajustes
net-blocked = Tor no consigue pasar
net-connecting = Conectando con Tor
net-bridged = A través de un puente
net-bridged-help = Esta red no ve que usas Tor.
net-connected = Conectado a Tor
net-connected-help = Los mensajes pasan por tres repetidores antes de llegar a nadie.

## La ruta

route-label = Ruta hacia { $name }
step-you = Tú
step-computer = Tu ordenador
step-computer-help = Los mensajes se cierran aquí antes de salir. Solo el dispositivo de { $name } puede abrirlos.
step-entrance = Entrada
step-private-entrance = Entrada privada
step-bridge-help = Un puente: una entrada a Tor que no está en la lista pública, así que esta red no puede distinguirla.
step-guard-help = El primer repetidor de Tor. Ve tu dirección, nunca adónde van los mensajes.
step-relay = Repetidor
step-relay-1-help = Se mantiene durante semanas: protege tu lado de la ruta. No puede leer el mensaje.
step-relay-2-help = Cambia cada pocas horas. No sabe ni quién escribe ni a quién.
step-meeting = Punto de encuentro
step-meeting-help = Donde tu ruta y la de { $name } se encuentran. Ninguno de los dos sabe dónde está el otro.
step-device-help = El dispositivo de { $name }, el único que puede abrir tus mensajes.
reaching = Buscando a { $name }…
connecting-percent = Conectando con Tor, { $percent } %
link-open = Conectado
link-idle = Disponible
link-away = No disponible ahora mismo
link-unknown = Conexión en cuanto escribas

## La conversación

empty-title = Aún no hay nadie
empty-help = Nightpost solo une a personas que se conocen. Envía a alguien un enlace de un solo uso, o léele un código por teléfono.
waiting-reachable = Se enviará cuando { $name } esté disponible
sending = Enviando…
about-name = Acerca de { $name }
verified = Verificado
verified-title = Número de seguridad comparado
verified-help = Los dos leísteis las mismas cifras: es { $name } de verdad.
muted = Silenciado
muted-until = Silenciado hasta las { $time }
muted-help = Los mensajes de { $name } siguen llegando, sin notificación.
read = Leído
delivered = Entregado
message-to = Mensaje para { $name }
write-to = Escribe a { $name }
send = Enviar
notify-from = Mensaje de { $name }
notify-new = Mensaje nuevo
notify-used-invite = { $name } ha usado tu invitación
now-contact = { $name } ya es un contacto
added = Contacto añadido: { $name }
kept = Contacto aceptado: { $name }
turned-down = Solicitud rechazada: { $name }
removed = Contacto eliminado: { $name }

## Añadir a alguien

add-title = Añadir a alguien
add-name-label = Su nombre, tal como lo verás
add-name-placeholder = Alicia
create-link = Crear un enlace
create-code = Crear un código para leer en voz alta
add-use-label = O usa lo que te dieron: su enlace, o el código que te leyeron
use-it = Usarlo
cancel = Cancelar
need-name = Primero ponle un nombre.
need-name-top = Primero ponle un nombre, arriba.
need-given = Pega su enlace, o escribe el código que te leyeron.
looking-title = Buscando a { $name }…
looking-tor = Esperando a Tor ({ $percent } %) antes de buscar.
looking-help = Tor está conectado. El Nightpost de { $name } tiene que estar abierto; encontrarlo puede tardar hasta 5 minutos.
looking-close = Cerrar esto no lo detiene: { $name } aparecerá en tu lista cuando se encuentre.
invite-title = Una invitación para { $name }
invite-help = Dásela solo a { $name }, por un medio de confianza. Sirve una vez, durante 7 días.
invite-qr = Código QR de la invitación
invite-link = Enlace de invitación
copy-link = Copiar el enlace
copy-code = Copiar el código
copied = Copiado
done = Hecho
hide = Ocultar
code-title = Lee este código a { $name }
code-help = Por teléfono o en persona. Vale durante 10 minutos.
code-after = Cuando { $name } lo escriba, compararéis un número de seguridad antes de guardar nada.
confirm-title = Alguien ha usado tu código para { $name }
confirm-help = Pídele que lea su número de seguridad en voz alta. El tuyo es:
confirm-warning = Acepta solo si todas las cifras coinciden. Si no, no es { $name }.
digits-match = Las cifras coinciden
digits-differ = No coinciden, rechazar
waiting-title = Esperando a { $name }
waiting-help = Lee este número de seguridad a { $name }, que comprueba que coincide con el suyo:
waiting-after = { $name } se añadirá en cuanto confirme.

## Número de seguridad

safety-label = Número de seguridad con { $name }
safety-title = Tu número de seguridad con { $name }
safety-help = Leedlo en voz alta, por teléfono o en persona. Si tenéis las mismas cifras, nadie se ha colado en medio.
they-match = Coinciden
not-now = Ahora no
safety-warning = Si una sola cifra es distinta, no es { $name } quien está al otro lado: no lo marques como verificado.

## El panel de una persona

change-photo = Cambiar la foto
choose-photo = Elegir una foto
use-rings = Usar los anillos
photo-help = La foto se queda en este ordenador. { $name } nunca la ve.
photo-failed = No se pudo usar esa imagen: { $error }
photo-too-big = ese archivo pasa de 30 MB
photo-too-detailed = esa imagen es demasiado detallada para guardarla
name = Nombre
rename = Cambiar nombre
name-help = Solo tú ves este nombre.
safety-number = Número de seguridad
not-compared = Aún sin comparar
compared-help = Lo comparaste con { $name } en persona o por teléfono.
compare-help = Compáralo una vez con { $name } para asegurarte de que nadie se ha colado en medio.
compare = Comparar el número de seguridad
notifications-from = Notificaciones de { $name }
mute-on = Activadas
mute-1h = 1 hora
mute-8h = 8 horas
mute-off = Desactivadas
mute-on-help = Una notificación por cada mensaje de { $name }.
mute-off-help = Ninguna notificación de { $name } hasta que las vuelvas a activar. Los mensajes siguen llegando.
mute-until-help = En silencio hasta las { $time }, luego se reactivan.
remove-title = ¿Eliminar a { $name }?
remove-help = { $name } ya no podrá contactarte ni ver cuándo estás conectado. Para volver a hablar, uno de los dos envía una nueva invitación.
remove-wipe = Borrar también la conversación de este ordenador
remove = Eliminar
remove-name = Eliminar a { $name }

## Ajustes

back = Volver a la conversación
settings-sections = Secciones de ajustes
appearance = Apariencia
network = Red
notifications = Notificaciones
security = Seguridad
about = Acerca de
theme = Tema
theme-system = Como el sistema
theme-dark = Oscuro
theme-light = Claro
theme-help = «Como el sistema» sigue el modo claro u oscuro de tu ordenador, y cambia con él.
language = Idioma
language-name = Español
language-help = «Como el sistema» usa el idioma de tu ordenador si Nightpost lo habla, y si no, el inglés.
connection = Conexión
bridges = Puentes
bridges-help = Algunas redes (wifi de la universidad o del trabajo) bloquean Tor. Nightpost entra entonces por un puente, una entrada que no está en la lista pública.
bridges-builtin = Integrados, cuando hagan falta
bridges-builtin-help = Snowflake, nada que configurar. Solo se usa si Tor no consigue pasar.
bridges-own = Tus propios puentes
bridges-own-help = De un amigo, de bridges.torproject.org (WebTunnel pasa casi siempre), o de tu propio servidor. Se usan siempre.
bridges-lines = Líneas de puente, una por línea
bridges-save = Guardar los puentes
bridges-reset = De vuelta a los puentes integrados.
bridges-saved = Puentes guardados: Tor se reconecta a través de ellos.
notify-show = Mostrar notificaciones
notify-show-help = Cuando llega un mensaje y Nightpost no está delante.
notify-sound = Reproducir un sonido
notify-sound-help = Un breve tono con cada notificación.
notify-history = Guardar en el historial
notify-history-help = Desactivado: cada notificación desaparece una vez mostrada, en lugar de quedarse en la lista del sistema.
notify-shows = Qué muestra una notificación
notify-who-what = Quién y qué
notify-who = Quién escribió
notify-nothing = Nada
notify-example-name = Lucía
notify-example-text = ¿Nos vemos a las ocho?
notify-locked = Una notificación puede aparecer en la pantalla bloqueada.
need-passphrase = Elige una frase de contraseña.
passphrases-differ = Las dos frases de contraseña no coinciden.
passphrase-saved = Frase de contraseña guardada
passphrase-removed = Frase de contraseña eliminada
locked-both = Con tu llavero y una frase de contraseña
locked-both-help = Tus mensajes y claves se guardan cifrados. Nightpost pide la frase de contraseña cada vez que se inicia.
locked-keychain = Con el llavero del sistema
locked-keychain-help = Tus mensajes y claves se guardan cifrados. La clave se abre cuando inicias sesión en este ordenador.
leave-lock-help = Los mensajes dejan de llegar, y se te pide la frase de contraseña al volver a abrir Nightpost.
leave-quit-help = Los mensajes dejan de llegar hasta que vuelvas a abrir Nightpost. Añade una frase de contraseña arriba para que esto lo bloquee.
how-locked = Cómo se protegen tus mensajes
passphrase-change = Cambiar la frase de contraseña
passphrase-remove = Quitar la frase de contraseña
passphrase-add = Añadir una frase de contraseña
passphrase-current = Frase de contraseña actual
passphrase-new = Nueva frase de contraseña
passphrase-again = Repítela
passphrase-warning = Si la olvidas, tus mensajes se pierden: nadie puede recuperarla, ni siquiera con este ordenador.
passphrase-remove-help = El llavero por sí solo volverá a abrir tus mensajes.
passphrase-add-it = Añadir la frase de contraseña
change-it = Cambiarla
remove-it = Quitarla
read-receipts = Confirmaciones de lectura
read-receipts-send = Enviar confirmaciones de lectura
read-receipts-help = Activado: la gente ve cuándo has leído sus mensajes, y tú ves cuándo leen los tuyos, si también lo tienen activado. Desactivado: ninguna de las dos cosas.
typing-indicators = Indicador de escritura
typing-indicators-send = Mostrar cuándo escribes
typing-indicators-help = Activado: la gente ve cuándo les escribes, y tú ves cuándo te escriben, si también lo tienen activado. Desactivado: ninguna de las dos cosas.
typing = { $name } está escribiendo…
leaving = Al cerrar
close-locks = Cerrar la ventana bloquea Nightpost
close-quits = Cerrar la ventana cierra Nightpost
quit-lock = Salir y bloquear
quit = Salir de Nightpost
tagline = Mensajes entre personas que se conocen, a través de Tor, sin ningún servidor en medio.
your-data = Tus datos
folder = Carpeta
open-folder = Abrir la carpeta
licences = Licencias
licence-bridges = lyrebird (puentes)
licence-icons = Iconos

## La interfaz de terminal (las teclas y, n, w y c se quedan como están)

weekday-short-1 = lun
weekday-short-2 = mar
weekday-short-3 = mié
weekday-short-4 = jue
weekday-short-5 = vie
weekday-short-6 = sáb
weekday-short-7 = dom
tui-no-daemon = No hay ningún daemon en marcha: inicia primero `nightpost daemon` (en tmux, por ejemplo).
tui-no-stream = El daemon no aceptó enviar sus eventos.
tui-help-hint = /help para las teclas y los comandos
tui-help-title = Teclas y comandos
tui-type-help = Escribe /help
tui-esc-closes = Esc cierra
tui-keys-hint = Enter envía, Alt+Enter empieza una línea nueva, Tab cambia de conversación
tui-link-copied = Enlace copiado (si el terminal lo permite)
tui-no-conversation = Aún no hay conversaciones: /invite o /code a alguien
tui-no-selection = Ninguna conversación seleccionada
tui-mute-usage = /mute, /mute 1h o /mute 8h
tui-muted-forever = { $name } silenciado hasta /unmute
tui-muted-until = { $name } silenciado hasta las { $time }
tui-unmuted = Las notificaciones de { $name } vuelven a estar activadas
tui-unknown-command = Comando desconocido: /help los muestra
tui-looking = Buscando (hasta unos minutos)…
tui-renamed = { $name } ahora es { $to }
tui-qr-too-small = El código QR necesita un terminal de { $width }×{ $height }: agrándalo o copia el enlace.
tui-invite-help = Dásela solo a { $name }. Sirve una vez, durante 7 días. c copia el enlace.
tui-confirm-keep = y  las cifras coinciden, aceptar
tui-confirm-refuse = n  no coinciden, rechazar
tui-verify-yes = y  coinciden, marcar como verificado
tui-remove-keep = y    eliminar, conservar la conversación en este ordenador
tui-remove-wipe = w    eliminar y borrar la conversación
tui-remove-no = n    conservar el contacto
tui-not-delivered = { $time } ○ aún no entregado
tui-delivered = { $time } ✓✓
tui-read = { $time } ✓✓ leído
help-send = enviar
help-newline = línea nueva
help-tab = conversación siguiente, anterior
help-scroll = desplazar
help-esc = cerrar esto, o vaciar el cuadro
help-quit = salir (el daemon sigue funcionando)
help-invite = un enlace de un solo uso, con su código QR
help-code = un código de 12 cifras para leer en voz alta
help-accept = usar un enlace
help-acceptcode = usar un código que te leyeron
help-verify = el número de seguridad de esta conversación
help-rename = solo tú lo ves
help-mute = ninguna notificación de este contacto (hasta /unmute)
help-remove = ya no puede contactarte

## La app de macOS

mac-locked = Tus mensajes están protegidos con una frase de contraseña.
mac-passphrase = Frase de contraseña
mac-message = Mensaje
mac-return-sends = Retorno envía; Opción-Retorno empieza una línea nueva.
mac-verified-help = Número de seguridad comparado: los dos leísteis las mismas cifras, es { $name } de verdad.
mac-muted-help = Silenciado: los mensajes de { $name } siguen llegando, sin notificación.
mac-theme-help = «Como el sistema» sigue el modo claro u oscuro de tu Mac, y cambia con él.
mac-locked-keychain-help = Tus mensajes y claves se guardan cifrados. La clave se abre cuando inicias sesión en este Mac.
mac-passphrase-warning = Si la olvidas, tus mensajes se pierden: nadie puede recuperarla, ni siquiera con este Mac.
mac-daemon-silent = El daemon de Nightpost no responde.
mac-show-in-finder = Mostrar en el Finder
mac-photo-not-picture = no es una imagen
mac-photo-resize = no se pudo redimensionar
mac-photo-reencode = no se pudo volver a codificar
