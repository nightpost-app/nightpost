# Nightpost, Deutsch. Übersetzung, von einer Muttersprachlerin oder einem Muttersprachler noch zu prüfen.
# Eine Nachricht pro Zeile; { $name } setzt das Programm ein.

## Daten

today = Heute
yesterday = Gestern
date-this-year = { $weekday }, { $day }. { $month }
date-other-year = { $day }. { $month } { $year }
date-short = { $day }. { $month }
weekday-1 = Montag
weekday-2 = Dienstag
weekday-3 = Mittwoch
weekday-4 = Donnerstag
weekday-5 = Freitag
weekday-6 = Samstag
weekday-7 = Sonntag
month-1 = Januar
month-2 = Februar
month-3 = März
month-4 = April
month-5 = Mai
month-6 = Juni
month-7 = Juli
month-8 = August
month-9 = September
month-10 = Oktober
month-11 = November
month-12 = Dezember
month-short-1 = Jan.
month-short-2 = Feb.
month-short-3 = März
month-short-4 = Apr.
month-short-5 = Mai
month-short-6 = Juni
month-short-7 = Juli
month-short-8 = Aug.
month-short-9 = Sep.
month-short-10 = Okt.
month-short-11 = Nov.
month-short-12 = Dez.

## Tor, wie der Daemon es meldet („Tor: “ bleibt, wie es ist: die Oberfläche sucht danach)

tor-percent = Tor: { $percent } %
tor-stuck = Tor: hängt? { $why }
tor-error = Tor: { $error }
tor-switch-failed = Tor: Umschalten fehlgeschlagen: { $error }
tor-reconnecting = Tor: verbindet neu…
tor-reconnecting-bridges = Tor: verbindet neu über die Brücken…
tor-to-bridge = Tor: in diesem Netz blockiert? Weiter über eine Brücke…
tor-from-bridge = Tor: die Brücke kommt nicht durch; Versuch ohne sie…

## Die Antworten des Daemons

err-starting = Tor ist noch nicht bereit: gleich noch einmal versuchen.
err-already-contact = Bereits ein Kontakt.
err-name-taken = { $name } ist bereits ein Kontakt.
err-code-waiting = Ein Code für { $name } wartet bereits.
err-not-confirmed = Nicht bestätigt.
err-did-not-confirm = { $name } hat nicht bestätigt.
err-nobody-waiting = Unter diesem Namen wartet niemand.
err-code-length = Ein Code hat 12 Ziffern.
err-code-typo = Dieser Code hat einen Tippfehler: eine Ziffer ist falsch oder zwei sind vertauscht.
err-not-found = In 5 Minuten nicht gefunden: läuft Nightpost auf der anderen Seite?
err-no-contact = Diesen Kontakt gibt es nicht.
err-invite-not = Das ist keine Nightpost-Einladung.
err-invite-version = Diese Einladung stammt aus einer anderen Version von Nightpost: einer von euch muss aktualisieren.
err-invite-malformed = Diese Einladung ist unvollständig oder falsch abgeschrieben.
err-invite-signature = Diese Einladung wurde verändert.
err-invite-expired = Diese Einladung ist abgelaufen: bitte um eine neue.
err-passphrase-keychain = Ohne Schlüsselbund kann die Passphrase hier nicht geändert werden.
err-passphrase-empty = Eine leere Passphrase schützt nichts.
err-passphrase-wrong = Das ist nicht die aktuelle Passphrase.
err-unexpected = Unerwartete Antwort: { $reply }

## Start

connecting-daemon = Verbindung zum Daemon…
starting = Startet…
restarting-update = Nightpost startet nach dem Update neu…
daemon-stopped = Der Daemon wurde beendet. Öffne Nightpost erneut, um ihn zu starten.
daemon-silent = Der Daemon von Nightpost antwortet nicht: { $error }
bridges-button = Tor hier blockiert? Brücken
unlock-prompt = Deine Passphrase öffnet deine Nachrichten.
unlock = Entsperren
wrong-passphrase = Diese Passphrase öffnet sie nicht. Versuch es noch einmal.

## Fenster

minimise = Minimieren
maximise = Maximieren
restore = Wiederherstellen
close = Schließen
show-list = Liste zeigen
hide-list = Liste ausblenden

## Die Liste der Personen

reachable-count = { $reachable } von { $total } erreichbar
preview-you = Du: { $text }
no-messages = Noch keine Nachrichten
add-someone = Jemanden hinzufügen
settings = Einstellungen
net-blocked = Tor kommt nicht durch
net-connecting = Verbindung zu Tor
net-bridged = Über eine Brücke
net-bridged-help = Dieses Netz sieht nicht, dass du Tor benutzt.
net-connected = Mit Tor verbunden
net-connected-help = Nachrichten laufen über drei Relays, bevor sie jemanden erreichen.

## Der Weg

route-label = Weg zu { $name }
step-you = Du
step-computer = Dein Computer
step-computer-help = Nachrichten werden hier verschlossen, bevor sie losgehen. Nur das Gerät von { $name } kann sie öffnen.
step-entrance = Eingang
step-private-entrance = Privater Eingang
step-bridge-help = Eine Brücke: ein Eingang ins Tor-Netz, der nicht auf der öffentlichen Liste steht, sodass dieses Netz ihn nicht erkennen kann.
step-guard-help = Das erste Tor-Relay. Es sieht deine Adresse, aber nie, wohin Nachrichten gehen.
step-relay = Relay
step-relay-1-help = Bleibt wochenlang: es schützt deine Seite des Wegs. Es kann die Nachricht nicht lesen.
step-relay-2-help = Wechselt alle paar Stunden. Es weiß weder, wer schreibt, noch an wen.
step-meeting = Treffpunkt
step-meeting-help = Wo sich dein Weg und der von { $name } treffen. Keiner von euch erfährt, wo der andere ist.
step-device-help = Das Gerät von { $name }, das einzige, das deine Nachrichten öffnen kann.
reaching = { $name } wird gesucht…
connecting-percent = Verbindung zu Tor, { $percent } %
link-open = Verbunden
link-idle = Erreichbar
link-away = Gerade nicht erreichbar
link-unknown = Verbindung, sobald du schreibst
link-older = { $name } muss Nightpost aktualisieren, um mit dir zu schreiben
link-newer = Aktualisiere Nightpost, um mit { $name } zu schreiben
other-older = Die andere Person muss Nightpost aktualisieren, bevor ihr schreiben könnt.
other-newer = Aktualisiere Nightpost: Die andere Person hat eine neuere Version.

## Die Unterhaltung

empty-title = Noch niemand hier
empty-help = Nightpost verbindet nur Menschen, die sich kennen. Schick jemandem einen Einmal-Link oder lies ihm am Telefon einen Code vor.
waiting-reachable = Wird gesendet, sobald { $name } erreichbar ist
sending = Wird gesendet…
about-name = Über { $name }
verified = Verifiziert
verified-title = Sicherheitsnummer verglichen
verified-help = Ihr habt beide dieselben Ziffern gelesen: das ist wirklich { $name }.
muted = Stummgeschaltet
muted-until = Stumm bis { $time }
muted-help = Nachrichten von { $name } kommen weiter an, ohne Mitteilung.
read = Gelesen
delivered = Zugestellt
message-to = Nachricht an { $name }
write-to = An { $name } schreiben
send = Senden
notify-from = Nachricht von { $name }
notify-new = Neue Nachricht
notify-used-invite = { $name } hat deine Einladung benutzt
now-contact = { $name } ist jetzt ein Kontakt
added = Kontakt hinzugefügt: { $name }
kept = Kontakt behalten: { $name }
turned-down = Anfrage abgelehnt: { $name }
removed = Kontakt entfernt: { $name }

## Jemanden hinzufügen

add-title = Jemanden hinzufügen
add-name-label = Der Name, unter dem du die Person sehen wirst
add-name-placeholder = Alice
create-link = Link erstellen
create-code = Code zum Vorlesen erstellen
add-use-label = Oder nutze, was du bekommen hast: einen Link oder einen vorgelesenen Code
use-it = Verwenden
cancel = Abbrechen
need-name = Gib zuerst einen Namen ein.
need-name-top = Gib zuerst oben einen Namen ein.
need-given = Füge den Link ein oder tippe den vorgelesenen Code.
looking-title = { $name } wird gesucht…
looking-tor = Warte auf Tor ({ $percent } %), bevor gesucht wird.
looking-help = Tor ist verbunden. Nightpost muss bei { $name } laufen; das Finden kann bis zu 5 Minuten dauern.
looking-close = Schließen hält die Suche nicht an: { $name } erscheint in deiner Liste, sobald gefunden.
invite-title = Eine Einladung für { $name }
invite-help = Gib sie nur { $name }, über einen Weg, dem du vertraust. Sie gilt einmal, 7 Tage lang.
invite-qr = QR-Code der Einladung
invite-link = Einladungslink
copy-link = Link kopieren
copy-code = Code kopieren
copied = Kopiert
done = Fertig
hide = Ausblenden
code-title = Lies { $name } diesen Code vor
code-help = Am Telefon oder persönlich. Er gilt 10 Minuten.
code-after = Wenn { $name } ihn eintippt, vergleicht ihr eine Sicherheitsnummer, bevor etwas gespeichert wird.
confirm-title = Jemand hat deinen Code für { $name } benutzt
confirm-help = Bitte die Person, ihre Sicherheitsnummer laut vorzulesen. Deine lautet:
confirm-warning = Behalte den Kontakt nur, wenn jede Ziffer übereinstimmt. Sonst ist es nicht { $name }.
digits-match = Die Ziffern stimmen überein
digits-differ = Sie weichen ab, ablehnen
waiting-title = Warten auf { $name }
waiting-help = Lies { $name } diese Sicherheitsnummer vor. Die andere Seite prüft, ob sie übereinstimmt:
waiting-after = { $name } wird hinzugefügt, sobald die Bestätigung kommt.

## Sicherheitsnummer

safety-label = Sicherheitsnummer mit { $name }
safety-title = Deine Sicherheitsnummer mit { $name }
safety-help = Lest sie euch am Telefon oder persönlich vor. Habt ihr dieselben Ziffern, hat sich niemand dazwischengeschaltet.
they-match = Sie stimmen überein
not-now = Nicht jetzt
safety-warning = Weicht auch nur eine Ziffer ab, ist am anderen Ende nicht { $name }: dann nicht als verifiziert markieren.

## Das Feld einer Person

change-photo = Foto ändern
choose-photo = Foto wählen
use-rings = Die Ringe verwenden
photo-help = Das Foto bleibt auf diesem Computer. { $name } sieht es nie.
photo-failed = Dieses Bild konnte nicht verwendet werden: { $error }
photo-too-big = diese Datei ist größer als 30 MB
photo-too-detailed = dieses Bild ist zu detailreich, um es zu speichern
name = Name
rename = Umbenennen
name-help = Nur du siehst diesen Namen.
safety-number = Sicherheitsnummer
not-compared = Nicht verglichen
compared-help = Du hast sie mit { $name } persönlich oder am Telefon verglichen.
compare-help = Vergleiche sie einmal mit { $name }, um sicherzugehen, dass sich niemand dazwischengeschaltet hat.
compare = Sicherheitsnummer vergleichen
notifications-from = Mitteilungen von { $name }
mute-on = An
mute-1h = 1 Std.
mute-8h = 8 Std.
mute-off = Aus
mute-on-help = Eine Mitteilung für jede Nachricht von { $name }.
mute-off-help = Keine Mitteilung von { $name }, bis du sie wieder einschaltest. Nachrichten kommen weiter an.
mute-until-help = Stumm bis { $time }, danach wieder an.
remove-title = { $name } entfernen?
remove-help = { $name } kann dich nicht mehr erreichen und nicht mehr sehen, wann du online bist. Um wieder zu schreiben, schickt einer von euch eine neue Einladung.
remove-wipe = Die Unterhaltung auch von diesem Computer löschen
remove = Entfernen
remove-name = { $name } entfernen

## Einstellungen

back = Zurück zur Unterhaltung
settings-sections = Bereiche der Einstellungen
appearance = Darstellung
network = Netzwerk
notifications = Mitteilungen
security = Sicherheit
about = Über
theme = Design
theme-system = Wie das System
theme-dark = Dunkel
theme-light = Hell
theme-help = „Wie das System“ folgt dem hellen oder dunklen Modus deines Computers und wechselt mit ihm.
language = Sprache
language-name = Deutsch
language-help = „Wie das System“ nimmt die Sprache deines Computers, wenn Nightpost sie spricht, sonst Englisch.
connection = Verbindung
bridges = Brücken
bridges-help = Manche Netze (WLAN an der Uni oder im Büro) blockieren Tor. Nightpost geht dann über eine Brücke hinein, einen Eingang, der nicht auf der öffentlichen Liste steht.
bridges-builtin = Eingebaut, bei Bedarf
bridges-builtin-help = Snowflake, nichts einzurichten. Nur genutzt, wenn Tor nicht durchkommt.
bridges-own = Eigene Brücken
bridges-own-help = Von Freunden, von bridges.torproject.org (WebTunnel kommt am häufigsten durch) oder vom eigenen Server. Werden immer genutzt.
bridges-lines = Brückenzeilen, eine pro Zeile
bridges-save = Brücken speichern
bridges-reset = Zurück zu den eingebauten Brücken.
bridges-saved = Brücken gespeichert: Tor verbindet sich über sie neu.
notify-show = Mitteilungen zeigen
notify-show-help = Wenn eine Nachricht ankommt und Nightpost nicht im Vordergrund ist.
notify-sound = Ton abspielen
notify-sound-help = Ein kurzer Klang bei jeder Mitteilung.
notify-history = Im Verlauf behalten
notify-history-help = Aus: Jede Mitteilung verschwindet nach dem Anzeigen, statt in der Liste des Systems zu bleiben.
notify-shows = Was eine Mitteilung zeigt
notify-who-what = Wer und was
notify-who = Wer geschrieben hat
notify-nothing = Nichts
notify-example-name = Lea
notify-example-text = Sehen wir uns um acht?
notify-locked = Eine Mitteilung kann auf dem Sperrbildschirm erscheinen.
need-passphrase = Wähle eine Passphrase.
passphrases-differ = Die beiden Passphrasen sind verschieden.
passphrase-saved = Passphrase gespeichert
passphrase-removed = Passphrase entfernt
locked-both = Mit deinem Schlüsselbund und einer Passphrase
locked-both-help = Deine Nachrichten und Schlüssel werden verschlüsselt gespeichert. Nightpost fragt bei jedem Start nach der Passphrase.
locked-keychain = Mit dem Schlüsselbund des Systems
locked-keychain-help = Deine Nachrichten und Schlüssel werden verschlüsselt gespeichert. Der Schlüssel öffnet sich, wenn du dich an diesem Computer anmeldest.
leave-lock-help = Nachrichten kommen nicht mehr an, und beim nächsten Öffnen von Nightpost wird nach deiner Passphrase gefragt.
leave-quit-help = Nachrichten kommen erst wieder an, wenn du Nightpost öffnest. Füge oben eine Passphrase hinzu, damit es dabei gesperrt wird.
how-locked = Wie deine Nachrichten verschlossen sind
passphrase-change = Passphrase ändern
passphrase-remove = Passphrase entfernen
passphrase-add = Passphrase hinzufügen
passphrase-current = Aktuelle Passphrase
passphrase-new = Neue Passphrase
passphrase-again = Wiederhole sie
passphrase-warning = Vergisst du sie, sind deine Nachrichten weg: niemand kann sie wiederherstellen, auch nicht mit diesem Computer.
passphrase-remove-help = Der Schlüsselbund allein öffnet deine Nachrichten dann wieder.
passphrase-add-it = Passphrase hinzufügen
change-it = Ändern
remove-it = Entfernen
read-receipts = Lesebestätigungen
read-receipts-send = Lesebestätigungen senden
read-receipts-help = An: andere sehen, wann du ihre Nachrichten gelesen hast, und du siehst, wann sie deine lesen, wenn sie es auch eingeschaltet haben. Aus: keins von beidem.
typing-indicators = Tippanzeige
typing-indicators-send = Anzeigen, wenn du schreibst
typing-indicators-help = An: andere sehen, wann du ihnen schreibst, und du siehst, wann sie dir schreiben, wenn sie es auch eingeschaltet haben. Aus: keins von beidem.
typing = { $name } schreibt…
leaving = Beim Schließen
close-locks = Fenster schließen sperrt Nightpost
close-quits = Fenster schließen beendet Nightpost
quit-lock = Beenden und sperren
quit = Nightpost beenden
tagline = Nachrichten zwischen Menschen, die sich kennen, über Tor, ohne Server dazwischen.
your-data = Deine Daten
folder = Ordner
open-folder = Ordner öffnen
licences = Lizenzen
licence-bridges = lyrebird (Brücken)
licence-icons = Symbole

## Die Terminal-Oberfläche (die Tasten y, n, w und c bleiben, wie sie sind)

weekday-short-1 = Mo.
weekday-short-2 = Di.
weekday-short-3 = Mi.
weekday-short-4 = Do.
weekday-short-5 = Fr.
weekday-short-6 = Sa.
weekday-short-7 = So.
tui-no-daemon = Kein Daemon läuft: starte zuerst `nightpost daemon` (zum Beispiel in tmux).
old-daemon = Der laufende Daemon hat Version { $running }, dieser { $mine }: beende ihn mit `nightpost shutdown` und starte `nightpost daemon` neu.
tui-no-stream = Der Daemon wollte keine Ereignisse senden.
tui-help-hint = /help für Tasten und Befehle
tui-help-title = Tasten und Befehle
tui-type-help = Tippe /help
tui-esc-closes = Esc schließt
tui-keys-hint = Enter sendet, Alt+Enter beginnt eine neue Zeile, Tab wechselt die Unterhaltung
tui-link-copied = Link kopiert (wenn das Terminal es erlaubt)
tui-no-conversation = Noch keine Unterhaltung: /invite oder /code jemanden
tui-no-selection = Keine Unterhaltung ausgewählt
tui-mute-usage = /mute, /mute 1h oder /mute 8h
tui-muted-forever = { $name } stumm bis /unmute
tui-muted-until = { $name } stumm bis { $time }
tui-unmuted = Mitteilungen von { $name } sind wieder an
tui-unknown-command = Unbekannter Befehl: /help zeigt alle
tui-looking = Suche läuft (bis zu einigen Minuten)…
tui-renamed = { $name } heißt jetzt { $to }
tui-qr-too-small = Der QR-Code braucht ein Terminal mit { $width }×{ $height }: vergrößere es oder kopiere den Link.
tui-invite-help = Gib sie nur { $name }. Sie gilt einmal, 7 Tage lang. c kopiert den Link.
tui-confirm-keep = y  die Ziffern stimmen, behalten
tui-confirm-refuse = n  sie weichen ab, ablehnen
tui-verify-yes = y  sie stimmen, als verifiziert markieren
tui-remove-keep = y    entfernen, Unterhaltung auf diesem Computer behalten
tui-remove-wipe = w    entfernen und Unterhaltung löschen
tui-remove-no = n    behalten
tui-not-delivered = { $time } ○ noch nicht zugestellt
tui-delivered = { $time } ✓✓
tui-read = { $time } ✓✓ gelesen
help-send = senden
help-newline = neue Zeile
help-tab = nächste, vorige Unterhaltung
help-scroll = blättern
help-esc = dies schließen oder die Eingabe leeren
help-quit = beenden (der Daemon läuft weiter)
help-invite = ein Einmal-Link mit QR-Code
help-code = ein 12-stelliger Code zum Vorlesen
help-accept = einen Link verwenden
help-acceptcode = einen vorgelesenen Code verwenden
help-verify = die Sicherheitsnummer dieser Unterhaltung
help-rename = nur du siehst ihn
help-mute = keine Mitteilungen von dieser Person (bis /unmute)
help-remove = die Person kann dich nicht mehr erreichen

## Die macOS-App

mac-locked = Deine Nachrichten sind mit einer Passphrase verschlossen.
mac-passphrase = Passphrase
mac-message = Nachricht
mac-return-sends = Return sendet; Wahl-Return beginnt eine neue Zeile.
mac-verified-help = Sicherheitsnummer verglichen: ihr habt beide dieselben Ziffern gelesen, das ist wirklich { $name }.
mac-muted-help = Stumm: Nachrichten von { $name } kommen weiter an, ohne Mitteilung.
mac-theme-help = „Wie das System“ folgt dem hellen oder dunklen Modus deines Mac und wechselt mit ihm.
mac-locked-keychain-help = Deine Nachrichten und Schlüssel werden verschlüsselt gespeichert. Der Schlüssel öffnet sich, wenn du dich an diesem Mac anmeldest.
mac-passphrase-warning = Vergisst du sie, sind deine Nachrichten weg: niemand kann sie wiederherstellen, auch nicht mit diesem Mac.
mac-daemon-silent = Der Daemon von Nightpost antwortet nicht.
mac-show-in-finder = Im Finder zeigen
mac-photo-not-picture = es ist kein Bild
mac-photo-resize = es konnte nicht verkleinert werden
mac-photo-reencode = es konnte nicht neu kodiert werden
