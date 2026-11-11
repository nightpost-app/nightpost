Nightpost on a USB key
======================

Start
  Double-click this computer's launcher:
    Nightpost Linux.sh · Nightpost Mac.command · Nightpost Windows.bat
  Your messages stay in the data/ folder, on the key.

First start
  Choose a passphrase. It is the only key to your messages: the computer's
  keychain is never used. Forgotten, it cannot be recovered, and there is
  no recovery phrase.

Close
  Closing the window stops Nightpost. Wait until it has closed, then eject
  the key before pulling it out.

What the key does not do
  - It does not protect you from a compromised computer: spyware sees what
    you type and read. Do not use it on a machine you doubt.
  - Nightpost writes nothing outside the key, apart from temporary files
    removed when it closes. The system may still note that a key was
    plugged in and a program started.
  - On Linux and macOS, Tor picks new entry guards at each start, as Tails does.

Per system
  - Linux: the window needs WebKitGTK 4.1 on the computer
    (package libwebkit2gtk-4.1). Without it, the terminal interface:
      NIGHTPOST_PORTABLE=1 NIGHTPOST_DIR=<key>/data <key>/linux/nightpost
  - macOS: on the first start, Gatekeeper may block the app. Right-click
    mac/Nightpost.app, then Open. Apple silicon Macs (M1 and later) only.
  - Windows: SmartScreen warns. "More info", then "Run anyway".
