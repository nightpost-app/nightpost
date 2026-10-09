# Where Tor is blocked: bridges

Some networks block Tor: many university Wi-Fi networks (eduroam among
them), offices, some countries. Nightpost then reaches Tor through a
*bridge*, an unlisted entry point the network does not know about.

There are three levels, from nothing to do to most robust.

## 1. Nothing to do: Snowflake

Nightpost comes with lyrebird, the Tor Project's bridge program, and Tor
Browser's built-in Snowflake bridges. When Tor makes no progress for 30
seconds, it switches to them on its own, and remembers it for the next start.
Snowflake looks like a video call and works on most blocked networks. It is
slow to start (a minute or two), and some networks still get in its way.

## 2. Bridges of your own

If Snowflake does not get through, give Nightpost other bridges:

- **Graphical app:** the *Bridges* button at the bottom of the list of people,
  or *Tor blocked here? Bridges* on the start screen while Tor connects. Paste
  the lines, *Save*: they are used at once.
- **Terminal:** put the lines in `bridges.txt` in the data directory. They
  are used within seconds, no restart needed.

  | System  | Data directory                              |
  |---------|---------------------------------------------|
  | Linux   | `~/.local/share/nightpost/`                 |
  | macOS   | `~/Library/Application Support/Nightpost/`  |
  | Windows | `%APPDATA%\Nightpost\data\`                 |

  One bridge per line; blank lines and lines starting with `#` are skipped.
  An empty file (or none) means Snowflake again.

Where to get bridges:

- <https://bridges.torproject.org/options>: pick **webtunnel** first (it
  looks like an ordinary HTTPS website and gets through most often), then
  obfs4.
- Telegram: [@GetBridgesBot](https://t.me/GetBridgesBot).
- Email `bridges@torproject.org` from a Gmail or Riseup address.
- A friend who runs a bridge (see 3).

A bridge line looks like this:

```
webtunnel [2001:db8::1]:443 0123456789ABCDEF0123456789ABCDEF01234567 url=https://example.com/secret-path ver=0.0.1
obfs4 203.0.113.7:443 0123456789ABCDEF0123456789ABCDEF01234567 cert=… iat-mode=0
```

## 3. A private bridge of your own

Bridges from the Tor Project are shared with many people, so the networks
that block Tor eventually learn some of them. A bridge you run yourself and
publish nowhere is known only to the people you give its line to: no list
can contain it.

This needs a machine that is always on (a home server, a Raspberry Pi, a
small VPS) and that the blocked network can reach.

### obfs4 with Docker

`docker-compose.yml`:

```yaml
services:
  tor-bridge:
    image: thetorproject/obfs4-bridge:latest
    container_name: tor-bridge
    restart: unless-stopped
    ports:
      - "9443:9443"
    environment:
      - OR_PORT=127.0.0.1:9001
      - PT_PORT=9443
      - EMAIL=none
      - NICKNAME=private
      - OBFS4_ENABLE_ADDITIONAL_VARIABLES=1
      # Private: never sent to the Tor directory, never handed out.
      - OBFS4V_PublishServerDescriptor=0
      - OBFS4V_BridgeDistribution=none
      - OBFS4V_AssumeReachable=1
      # Tor counts its compression buffers (~300 MB) against this: lower,
      # it runs "low on memory" and drops every circuit, yours included.
      - OBFS4V_MaxMemInQueues=1024 MB
    volumes:
      - data:/var/lib/tor

volumes:
  data:
```

1. `docker compose up -d`, then wait for `Bootstrapped 100%` in
   `docker logs tor-bridge`.
2. Get the two halves of the bridge line:

   ```
   docker exec tor-bridge cat /var/lib/tor/pt_state/obfs4_bridgeline.txt
   docker exec tor-bridge cat /var/lib/tor/fingerprint
   ```

   The first gives `Bridge obfs4 <IP ADDRESS>:<PORT> <FINGERPRINT> cert=… iat-mode=0`,
   the second `private <FINGERPRINT>`.
3. Write the line with the address the blocked network will reach, the
   port, and the fingerprint, without the leading `Bridge`:

   ```
   obfs4 198.51.100.20:9443 0123456789ABCDEF0123456789ABCDEF01234567 cert=… iat-mode=0
   ```

4. Paste it in Nightpost (see 2).

### Making it reachable

- **Public address:** forward TCP port 9443 on your router to the machine
  (or open it in the VPS firewall), and use your public IP address. A port
  such as 443 gets through more filters, if nothing else uses it.
- **Through a VPN you already have** (Tailscale, WireGuard): use the
  machine's address on that VPN. Nothing to open, but it only works while that
  VPN itself gets through the network.

### Good to know

- Anyone with the line can use the bridge and its bandwidth: give it only to
  people you trust, through Nightpost itself or in person.
- The bridge only carries encrypted Tor traffic to the Tor network; it does
  not see what goes through it, nor where.
- obfs4 traffic looks random, which some filters notice. WebTunnel, which
  looks like a website, is harder to tell apart but needs a domain name and an
  HTTPS server in front: see the Tor Project's guide,
  <https://community.torproject.org/relay/setup/webtunnel/>. (Nightpost
  supports WebTunnel lines; we have only tested our own bridge with obfs4.)

## If it still does not connect

The daemon's log says what Tor is doing: lines starting with `Tor:`. The
graphical app writes it to `daemon.log` in the data directory.

- `Tor: blocked on this network? Going through a bridge…`: the switch
  happened.
- `Tor: reconnecting through the bridges…`: your lines were picked up.
- `Tor: bridge line "…": …`: a line has a typo; nothing changed.
- `no lyrebird next to nightpost nor on the PATH`: keep `lyrebird` in the same
  folder as `nightpost`, as the release archives ship it.
