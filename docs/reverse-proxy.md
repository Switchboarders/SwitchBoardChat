**English** · [Français](fr/reverse-proxy.md)

# Putting switchboard-chat behind a reverse proxy

The bridge speaks plain HTTP on port **8088** (and HTTPS with a self-signed certificate on **3443**,
for tests). In production, a reverse proxy in front provides the public name and a real TLS
certificate. Any proxy works if it meets the requirements below; examples follow.

## Requirements

1. **HTTPS to the browser.** The website is served over HTTPS, so browsers refuse calls to plain HTTP
   (and the legacy cookie, if enabled, is `Secure`).
2. **Do not buffer `/web/stream`.** Replies are pushed as server-sent events (SSE); a buffering proxy
   holds them back, so the visitor never sees an answer. This is the most common failure.
3. **Long read timeout** for `/web/stream` (the bridge sends a keep-alive comment every 15 s, so
   anything above ~60 s works; 1 hour is comfortable).
4. **Forward the client address** with `X-Forwarded-For`, **appended by the proxy**. The bridge uses the
   *last* entry for per-visitor rate limits (so a client cannot spoof it by sending its own header).
   If a CDN sits in front of your proxy, the last entry is the CDN's address; in that case have the
   proxy set the header from the CDN's real-client header, or accept that limits apply per edge.
5. **Do not add CORS headers in the proxy.** The bridge answers CORS itself, with the exact origin
   and `Allow-Credentials`. Duplicate headers break browsers. The proxy must pass the `Authorization`
   header through (the default).
6. **Do not compress** `text/event-stream`.
7. **Keep port 8088 private** (firewall or an internal network): it has no TLS and trusts
   `X-Forwarded-For`. Only this listener reads that header (port 3443 ignores it). Set
   `TRUSTED_PROXIES` to your proxy's address or network (e.g. `172.18.0.0/16`) so that no other
   container on the same network can claim another visitor's address, and `HTTP_BIND`
   (default `0.0.0.0:8088`) to listen on one interface only.
8. Only `/web/*` needs these settings; the rest of the bridge (`/voip_api`, `/sbchat_api`) works
   with an ordinary proxy rule, so one `server` can serve all of it.

Replace `chat.example.org` with your name and `127.0.0.1:8088` with the bridge's address (a
container name such as `sb-chat:8088` on a shared Docker network works too).

## nginx

```nginx
server {
    listen 443 ssl;
    server_name chat.example.org;
    # ssl_certificate / ssl_certificate_key …

    # Web chat: streaming, no buffering
    location /web/ {
        proxy_pass http://127.0.0.1:8088;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Connection "";
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 1h;
    }

    # Everything else (SMS webhook, admin API)
    location / {
        proxy_pass http://127.0.0.1:8088;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

If you have several `server` blocks for the same bridge under different names, **each** one needs
the `location /web/` block: a block in a different `server` does not apply.
Test the configuration with `nginx -t`, reload with `nginx -s reload` (restart the container if the
config file is a single-file bind mount that your editor replaced).

## Caddy

```caddyfile
chat.example.org {
    # Caddy provisions the certificate and forwards X-Forwarded-For by itself
    reverse_proxy 127.0.0.1:8088 {
        flush_interval -1        # stream immediately (SSE)
    }
}
```

## Apache httpd (2.4)

Enable `mod_proxy`, `mod_proxy_http`, `mod_headers`, `mod_ssl`.

```apache
<VirtualHost *:443>
    ServerName chat.example.org
    # SSLEngine on / SSLCertificateFile … / SSLCertificateKeyFile …

    ProxyPreserveHost On
    RequestHeader set X-Forwarded-Proto "https"

    # Web chat: stream without buffering, long timeout, no compression
    <Location "/web/">
        SetEnv no-gzip 1
        ProxyPass        "http://127.0.0.1:8088/web/" flushpackets=on timeout=3600
        ProxyPassReverse "http://127.0.0.1:8088/web/"
    </Location>

    ProxyPass        "/" "http://127.0.0.1:8088/"
    ProxyPassReverse "/" "http://127.0.0.1:8088/"
</VirtualHost>
```

`mod_proxy` adds `X-Forwarded-For` by itself. Verify streaming with the test below: some Apache
setups need the `mod_deflate` filter excluded for `text/event-stream`.

## Traefik (v2/v3, dynamic file configuration)

```yaml
http:
  routers:
    sbchat:
      rule: Host(`chat.example.org`)
      entryPoints: [websecure]
      tls: { certResolver: letsencrypt }
      service: sbchat
  services:
    sbchat:
      loadBalancer:
        servers:
          - url: http://127.0.0.1:8088
```

Traefik appends `X-Forwarded-For` and flushes `text/event-stream` responses immediately. Do not attach
a compression middleware to this router.

## Hosting panels, CDNs, other proxies

- **Shared hosting / panels:** if you cannot change the proxy, the chat cannot be offered from that
  host. Run the bridge on a VPS or a machine you control and point the website at it.
- **CDN in front (Cloudflare and similar):** make sure `/web/*` is not cached and not buffered,
  and run the live stream test below. Keep the chat on a sub-domain of the website.
- **HAProxy / cloud load balancers:** raise the client/server timeouts above 60 s for this route
  and disable response buffering/compression for `text/event-stream`.

## Test it

From any machine (use your real name and website origin):

```bash
# 1. HTTPS and CORS
curl -si -X POST https://chat.example.org/web/session -H 'Origin: https://www.example.org' | head -12

# 2. Streaming: a ":" line must arrive within ~15 s
TOKEN=$(curl -s -X POST -H 'Origin: https://www.example.org' https://chat.example.org/web/session | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
curl -sN -m 25 -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/stream
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/end
```

Step 2 printing nothing within 25 s means the proxy buffers; check requirements 2, 3 and 6.
A `502` means the proxy cannot reach the bridge. Each test session counts against the per-IP limit
of 5 new sessions per hour.

## A note on cookies

Since widget 0.5.0 the chat uses **no cookie**: the session token travels in an `Authorization`
header, so the chat works whatever browsers' third-party cookie settings are (Safari, Firefox strict
mode, Samsung Internet) and the chat server may be on any domain. Only the legacy cookie mode
(`WEB_SESSION_COOKIE=true`, for widgets older than 0.5.0) still needs the chat name to be under the
website's own registrable domain (website `www.example.org`, chat `chat.example.org`).
