# A mailbox for your account

Nightpost has no server: a message goes straight from one device to
another, over Tor. When one of your devices is off, a contact's message
waits on their side until it comes back. If they go offline in the meantime,
the message waits longer.

A mailbox fixes that. It is one more device on your account, on a machine
that stays on (a home server, a small VPS). When one of your devices does
not answer, your contacts leave the message at the mailbox, already
encrypted for that device. The device collects it when it comes back.

## What it sees, and what it does not

- It **never** holds your identity key or your discovery key. Someone who
  takes the machine cannot read your messages or write as you.
- It cannot read what it keeps: each message is encrypted for one of your
  devices.
- It knows your contacts' public keys (to let them in), which of them writes
  to which of your devices, and when. It does not know the names you gave
  them.
- It keeps a message until the device it is for collects it, for 30 days at
  most. Each contact can leave at most 2,000 messages and 20 MiB, and the
  mailbox holds 1 GiB in all.

## Setting it up (Linux with systemd)

1. Install the `nightpost-cli` package on the server.
2. Choose the passphrase that encrypts the mailbox's database:

   ```
   sudo install -d -m 700 /etc/nightpost
   head -c 32 /dev/urandom | base64 | sudo tee /etc/nightpost/mailbox-passphrase > /dev/null
   sudo chmod 600 /etc/nightpost/mailbox-passphrase
   ```

3. Start it:

   ```
   sudo systemctl enable --now nightpost-mailbox
   ```

4. Read the pairing link it shows:

   ```
   journalctl -u nightpost-mailbox -n 50
   ```

5. On one of your devices, within 10 minutes:

   ```
   nightpost devices add nightpost:device:…
   ```

   It then appears in `nightpost devices` as a mailbox. If the link
   expires first, the mailbox restarts with a new one.

Without systemd, run it the same way: start `nightpost daemon --mailbox` with
`NIGHTPOST_DIR` and `NIGHTPOST_PASSPHRASE_FILE` set.

## Taking it off

`nightpost devices remove <id>` on one of your devices. Your contacts stop
leaving messages there once they learn your new device list. Then stop the
service and delete `/var/lib/private/nightpost-mailbox`.
