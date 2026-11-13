[English](../reverse-proxy.md) · **Français**

# Placer switchboard-chat derrière un proxy inverse

La passerelle parle en HTTP simple sur le port **8088** (et en HTTPS avec un certificat autosigné sur
le port **3443**, pour les essais). En production, un proxy inverse placé devant fournit le nom public
et un vrai certificat TLS. Tout proxy convient s’il respecte les exigences ci-dessous; des exemples
suivent.

## Exigences

1. **HTTPS jusqu’au navigateur.** Le site Web est servi en HTTPS : les navigateurs refusent donc les appels
   en HTTP simple (et l’ancien témoin, s’il est activé, est `Secure`).
2. **Ne pas mettre `/web/stream` en mémoire tampon.** Les réponses sont poussées en évènements envoyés
   par le serveur (SSE); un proxy qui met les données en tampon les retient, et le visiteur ne voit
   jamais de réponse. C’est la panne la plus courante.
3. **Délai de lecture long** pour `/web/stream` (la passerelle envoie un commentaire de maintien de
   connexion toutes les 15 s; tout délai supérieur à environ 60 s convient, 1 heure est confortable).
4. **Transmettre l’adresse du client** avec `X-Forwarded-For`, **ajoutée par le proxy**. La passerelle
   utilise la *dernière* entrée pour les limites de débit par visiteur (un client ne peut donc pas la
   falsifier en envoyant son propre en-tête). Si un réseau de diffusion de contenu (CDN) se trouve
   devant votre proxy, la dernière entrée est l’adresse du CDN; dans ce cas, faites en sorte que le
   proxy définisse l’en-tête à partir de l’en-tête « vrai client » du CDN, ou acceptez que les limites
   s’appliquent par point de présence.
5. **Ne pas ajouter d’en-têtes CORS dans le proxy.** La passerelle répond elle-même au CORS, avec
   l’origine exacte et `Allow-Credentials`. Des en-têtes en double font échouer les navigateurs. Le proxy doit
   laisser passer l’en-tête `Authorization` (c’est le comportement par défaut).
6. **Ne pas compresser** `text/event-stream`.
7. **Garder le port 8088 privé** (pare-feu ou réseau interne) : il n’a pas de TLS et fait confiance à
   `X-Forwarded-For`. Seul cet écouteur lit cet en-tête (le port 3443 l’ignore). Indiquez dans
   `TRUSTED_PROXIES` l’adresse ou le réseau de votre proxy (p. ex. `172.18.0.0/16`) pour qu’aucun autre
   conteneur du même réseau ne puisse se faire passer pour un autre visiteur, et dans `HTTP_BIND`
   (par défaut `0.0.0.0:8088`) une seule interface d’écoute.
8. Seul `/web/*` a besoin de ces réglages; le reste de la passerelle (`/voip_api`, `/sbchat_api`)
   fonctionne avec une règle de proxy ordinaire, donc un seul `server` peut tout servir.

Remplacez `chat.example.org` par votre nom et `127.0.0.1:8088` par l’adresse de la passerelle (un nom
de conteneur comme `sb-chat:8088` sur un réseau Docker partagé convient aussi).

## nginx

```nginx
server {
    listen 443 ssl;
    server_name chat.example.org;
    # ssl_certificate / ssl_certificate_key …

    # Clavardage Web : diffusion en continu, sans mémoire tampon
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

    # Tout le reste (webhook SMS, API d’administration)
    location / {
        proxy_pass http://127.0.0.1:8088;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Si vous avez plusieurs blocs `server` pour la même passerelle sous des noms différents, **chacun** a
besoin du bloc `location /web/` : un bloc placé dans un autre `server` ne s’applique pas.
Testez la configuration avec `nginx -t`, rechargez avec `nginx -s reload` (redémarrez le conteneur si
le fichier de configuration est un montage de fichier unique que votre éditeur a remplacé).

## Caddy

```caddyfile
chat.example.org {
    # Caddy obtient le certificat et transmet X-Forwarded-For de lui-même
    reverse_proxy 127.0.0.1:8088 {
        flush_interval -1        # diffuser immédiatement (SSE)
    }
}
```

## Apache httpd (2.4)

Activez `mod_proxy`, `mod_proxy_http`, `mod_headers`, `mod_ssl`.

```apache
<VirtualHost *:443>
    ServerName chat.example.org
    # SSLEngine on / SSLCertificateFile … / SSLCertificateKeyFile …

    ProxyPreserveHost On
    RequestHeader set X-Forwarded-Proto "https"

    # Clavardage Web : diffusion sans mémoire tampon, long délai, sans compression
    <Location "/web/">
        SetEnv no-gzip 1
        ProxyPass        "http://127.0.0.1:8088/web/" flushpackets=on timeout=3600
        ProxyPassReverse "http://127.0.0.1:8088/web/"
    </Location>

    ProxyPass        "/" "http://127.0.0.1:8088/"
    ProxyPassReverse "/" "http://127.0.0.1:8088/"
</VirtualHost>
```

`mod_proxy` ajoute `X-Forwarded-For` de lui-même. Vérifiez la diffusion avec l’essai ci-dessous :
certaines configurations d’Apache exigent d’exclure le filtre `mod_deflate` pour `text/event-stream`.

## Traefik (v2/v3, configuration dynamique par fichier)

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

Traefik ajoute `X-Forwarded-For` et diffuse immédiatement les réponses `text/event-stream`. N’associez
pas d’intergiciel de compression à ce routeur.

## Panneaux d’hébergement, CDN, autres proxys

- **Hébergement partagé / panneaux de contrôle :** si vous ne pouvez pas modifier le proxy, le
  clavardage ne peut pas être offert depuis cet hôte. Faites tourner la passerelle sur un VPS ou un
  poste que vous contrôlez et pointez le site Web vers lui.
- **CDN devant le serveur (Cloudflare et similaires) :** assurez-vous que `/web/*` n’est ni mis en
  cache ni mis en tampon, et faites l’essai de diffusion ci-dessous. Gardez le clavardage sur un
  sous-domaine du site Web.
- **HAProxy / répartiteurs de charge infonuagiques :** portez les délais client/serveur au-delà de 60 s
  pour cette route et désactivez la mise en tampon et la compression des réponses `text/event-stream`.

## Essai

Depuis n’importe quel poste (avec votre vrai nom et l’origine de votre site Web) :

```bash
# 1. HTTPS et CORS
curl -si -X POST https://chat.example.org/web/session -H 'Origin: https://www.example.org' | head -12

# 2. Diffusion : une ligne « : » doit arriver en ~15 s
TOKEN=$(curl -s -X POST -H 'Origin: https://www.example.org' https://chat.example.org/web/session | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
curl -sN -m 25 -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/stream
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Origin: https://www.example.org' https://chat.example.org/web/end
```

Si l’étape 2 n’affiche rien en 25 s, le proxy met les données en tampon; vérifiez les exigences 2, 3
et 6. Un `502` signifie que le proxy ne joint pas la passerelle. Chaque session d’essai compte dans la
limite de 5 nouvelles sessions par heure par adresse IP.

## À propos des témoins

Depuis la version 0.5.0 du module, le clavardage n’utilise **aucun témoin** : le jeton de session circule
dans un en-tête `Authorization`; le clavardage fonctionne donc quels que soient les réglages de témoins tiers
des navigateurs (Safari, Firefox en mode strict, Samsung Internet) et le serveur de clavardage peut se trouver
sur n’importe quel domaine. Seul l’ancien mode par témoin (`WEB_SESSION_COOKIE=true`, pour les modules
antérieurs à 0.5.0) exige encore que le nom du clavardage soit sous le domaine enregistré du site Web (site
`www.example.org`, clavardage `chat.example.org`).
