# Nightpost, English: the reference every other language follows.
# One message per line; { $name } is filled in by the program.

## Dates

today = Today
yesterday = Yesterday
# "Monday 5 October"
date-this-year = { $weekday } { $day } { $month }
# "5 October 2025"
date-other-year = { $day } { $month } { $year }
# In the list of people: "5 Oct"
date-short = { $day } { $month }
weekday-1 = Monday
weekday-2 = Tuesday
weekday-3 = Wednesday
weekday-4 = Thursday
weekday-5 = Friday
weekday-6 = Saturday
weekday-7 = Sunday
month-1 = January
month-2 = February
month-3 = March
month-4 = April
month-5 = May
month-6 = June
month-7 = July
month-8 = August
month-9 = September
month-10 = October
month-11 = November
month-12 = December
month-short-1 = Jan
month-short-2 = Feb
month-short-3 = Mar
month-short-4 = Apr
month-short-5 = May
month-short-6 = Jun
month-short-7 = Jul
month-short-8 = Aug
month-short-9 = Sep
month-short-10 = Oct
month-short-11 = Nov
month-short-12 = Dec

## Tor, as the daemon reports it ("Tor: " stays as is in every language: the interface looks for it)

tor-percent = Tor: { $percent }%
tor-stuck = Tor: stuck? { $why }
tor-error = Tor: { $error }
tor-switch-failed = Tor: switching failed: { $error }
tor-reconnecting = Tor: reconnecting…
tor-reconnecting-bridges = Tor: reconnecting through the bridges…
tor-to-bridge = Tor: blocked on this network? Going through a bridge…
tor-from-bridge = Tor: the bridge is not getting through; trying without it…

## The daemon's answers

err-starting = Tor is not up yet: try again in a moment.
err-already-contact = Already a contact.
err-name-taken = { $name } is already a contact.
err-code-waiting = A code for { $name } is already waiting.
err-not-confirmed = Not confirmed.
err-did-not-confirm = { $name } did not confirm.
err-nobody-waiting = Nobody is waiting under that name.
err-code-length = A code is 12 digits.
err-code-typo = This code has a typo: one digit is wrong or two are swapped.
err-not-found = Not found in 5 minutes: is their Nightpost running?
err-no-contact = No such contact.
err-invite-not = This is not a Nightpost invitation.
err-invite-version = This invitation comes from another version of Nightpost: one of you has to update.
err-invite-malformed = This invitation is cut short or mistyped.
err-invite-signature = This invitation has been tampered with.
err-invite-expired = This invitation has expired: ask for a new one.
err-passphrase-keychain = Without a keychain, the passphrase cannot be changed here.
err-passphrase-empty = An empty passphrase protects nothing.
err-passphrase-wrong = That is not the current passphrase.
err-unexpected = Unexpected reply: { $reply }

## Starting

connecting-daemon = Connecting to the daemon…
starting = Starting…
restarting-update = Restarting Nightpost after the update…
daemon-stopped = The daemon stopped. Reopen Nightpost to start it again.
daemon-silent = Nightpost's daemon does not answer: { $error }
bridges-button = Tor blocked here? Bridges
unlock-prompt = Your passphrase opens your messages.
unlock = Unlock
wrong-passphrase = That passphrase does not open them. Try again.

## Window

minimise = Minimise
maximise = Maximise
restore = Restore
close = Close
show-list = Show the list
hide-list = Hide the list

## The list of people

reachable-count = { $reachable } reachable of { $total }
preview-you = You: { $text }
no-messages = No messages yet
add-someone = Add someone
settings = Settings
net-blocked = Tor is not getting through
net-connecting = Connecting to Tor
net-bridged = Through a bridge
net-bridged-help = This network does not see that you use Tor.
net-connected = Connected to Tor
net-connected-help = Messages pass three relays before they reach anyone.

## The route

route-label = Route to { $name }
step-you = You
step-computer = Your computer
step-computer-help = Messages are locked here before they leave. Only { $name }’s device can open them.
step-entrance = Entrance
step-private-entrance = Private entrance
step-bridge-help = A bridge: a way into Tor that is not on the public list, so this network cannot tell it apart.
step-guard-help = The first Tor relay. It sees your address, never where messages go.
step-relay = Relay
step-relay-1-help = Kept for weeks: it guards your side of the route. It cannot read the message.
step-relay-2-help = Changes every few hours. It knows neither who writes nor to whom.
step-meeting = Meeting point
step-meeting-help = Where your route and { $name }’s meet. Neither of you learns where the other is.
step-device-help = { $name }’s device, the only one that can open your messages.
reaching = Reaching { $name }…
connecting-percent = Connecting to Tor, { $percent }%
link-open = Connected
link-idle = Reachable
link-away = Not reachable right now
link-unknown = Reached when you write

## The conversation

empty-title = Nobody here yet
empty-help = Nightpost only connects people who know each other. Send someone a one-time link, or read them a code on the phone.
waiting-reachable = Will be sent once { $name } is reachable
sending = Sending…
about-name = About { $name }
verified = Verified
verified-title = Safety number compared
verified-help = You both read the same digits: this is really { $name }.
muted = Muted
muted-until = Muted until { $time }
muted-help = Messages from { $name } still arrive, without a notification.
read = Read
delivered = Delivered
message-to = Message to { $name }
write-to = Write to { $name }
send = Send
notify-from = Message from { $name }
notify-new = New message
notify-used-invite = { $name } used your invitation
now-contact = { $name } is now a contact
added = { $name } added
kept = { $name } kept
turned-down = { $name } turned down
removed = { $name } removed

## Adding someone

add-title = Add someone
add-name-label = Their name, as you will see it
add-name-placeholder = Alice
create-link = Create a link
create-code = Create a code to read out
add-use-label = Or use what they gave you: their link, or the code they read you
use-it = Use it
cancel = Cancel
need-name = Give them a name first.
need-name-top = Give them a name first, at the top.
need-given = Paste their link, or type the code they read you.
looking-title = Looking for { $name }…
looking-tor = Waiting for Tor ({ $percent }%) before looking.
looking-help = Tor is connected. { $name }'s Nightpost has to be running; finding it can take up to 5 minutes.
looking-close = Closing this does not stop it: { $name } shows up in your list once found.
invite-title = An invitation for { $name }
invite-help = Give it to { $name } only, through a channel you trust. It works once, for 7 days.
invite-qr = QR code of the invitation
invite-link = Invitation link
copy-link = Copy the link
copy-code = Copy the code
copied = Copied
done = Done
hide = Hide
code-title = Read this code to { $name }
code-help = On a call or in person. It works for 10 minutes.
code-after = When { $name } types it in, you will both compare a safety number before anything is kept.
confirm-title = Someone used your code for { $name }
confirm-help = Ask them to read their safety number aloud. Yours is:
confirm-warning = Keep them only if every digit matches. Otherwise it is not { $name }.
digits-match = The digits match
digits-differ = They differ, turn down
waiting-title = Waiting for { $name }
waiting-help = Read this safety number to { $name }. They check that it matches theirs:
waiting-after = { $name } is added as soon as they confirm.

## Safety number

safety-label = Safety number with { $name }
safety-title = Your safety number with { $name }
safety-help = Read it to each other on a call or in person. The same digits on both sides mean nobody sits in between.
they-match = They match
not-now = Not now
safety-warning = If even one digit differs, it is not { $name } at the other end: do not mark it.

## Someone's panel

change-photo = Change photo
choose-photo = Choose a photo
use-rings = Use the rings
photo-help = The photo stays on this computer. { $name } never sees it.
photo-failed = That picture could not be used: { $error }
photo-too-big = That file is over 30 MB.
photo-too-detailed = That picture is too detailed to keep.
name = Name
rename = Rename
name-help = Only you see this name.
safety-number = Safety number
not-compared = Not compared yet
compared-help = You compared it with { $name } in person or on a call.
compare-help = Compare it once with { $name } to be sure nobody sits in between.
compare = Compare safety number
notifications-from = Notifications from { $name }
mute-on = On
mute-1h = 1 hour
mute-8h = 8 hours
mute-off = Off
mute-on-help = A notification for each message from { $name }.
mute-off-help = No notification from { $name } until you turn them back on. Messages still arrive.
mute-until-help = Quiet until { $time }, then back on.
remove-title = Remove { $name }?
remove-help = { $name } can no longer reach you or see when you are online. To talk again, one of you sends a new invitation.
remove-wipe = Also delete the conversation from this computer
remove = Remove
remove-name = Remove { $name }

## Settings

back = Back to the conversation
settings-sections = Settings sections
appearance = Appearance
network = Network
notifications = Notifications
security = Security
about = About
theme = Theme
theme-system = As the system
theme-dark = Dark
theme-light = Light
theme-help = “As the system” follows your computer’s light or dark mode, and changes with it.
connection = Connection
bridges = Bridges
bridges-help = Some networks (university or work Wi-Fi) block Tor. Nightpost then goes in through a bridge, an entrance that is not on the public list.
bridges-builtin = Built-in, when needed
bridges-builtin-help = Snowflake, nothing to set up. Used only if Tor does not get through.
bridges-own = Bridges of your own
bridges-own-help = From a friend, bridges.torproject.org (WebTunnel gets through most often), or your own server. Used all the time.
bridges-lines = Bridge lines, one per line
bridges-save = Save bridges
bridges-reset = Back to the built-in bridges.
bridges-saved = Bridges saved: Tor reconnects through them.
notify-show = Show notifications
notify-show-help = When a message arrives and Nightpost is not in front.
notify-sound = Play a sound
notify-sound-help = A short chime with each notification.
notify-history = Keep in history
notify-history-help = Off: the system lets each one go once shown, rather than keeping it in its list of past notifications.
notify-shows = What a notification shows
notify-who-what = Who and what
notify-who = Who wrote
notify-nothing = Nothing
notify-example-name = Léa
notify-example-text = See you at eight?
notify-locked = A notification can show on a locked screen.
need-passphrase = Choose a passphrase.
passphrases-differ = The two passphrases differ.
passphrase-saved = Passphrase saved
passphrase-removed = Passphrase removed
locked-both = With your keychain and a passphrase
locked-both-help = Your messages and keys are stored encrypted. Nightpost asks for the passphrase each time it starts.
locked-keychain = With your system keychain
locked-keychain-help = Your messages and keys are stored encrypted. The key opens when you log in to this computer.
leave-lock-help = Messages stop arriving, and your passphrase is asked for when you open Nightpost again.
leave-quit-help = Messages stop arriving until you open Nightpost again. Add a passphrase above for this to lock it.
how-locked = How your messages are locked
passphrase-change = Change the passphrase
passphrase-remove = Remove the passphrase
passphrase-add = Add a passphrase
passphrase-current = Current passphrase
passphrase-new = New passphrase
passphrase-again = The same again
passphrase-warning = Forget it and your messages are gone: nobody can recover it, not even with this computer.
passphrase-remove-help = The keychain alone will open your messages again.
passphrase-add-it = Add the passphrase
change-it = Change it
remove-it = Remove it
read-receipts = Read receipts
read-receipts-send = Send read receipts
read-receipts-help = On: people see when you have read their messages, and you see when they read yours, if they have it on too. Off: neither.
leaving = Leaving
close-locks = Closing the window locks Nightpost
close-quits = Closing the window quits Nightpost
quit-lock = Quit and lock
quit = Quit Nightpost
tagline = Messages between people who know each other, over Tor, with no server in between.
your-data = Your data
folder = Folder
open-folder = Open the folder
licences = Licences
licence-bridges = lyrebird (bridges)
licence-icons = Icons
