# Server setup — scoped deploy user

One-time VPS setup. Goal: CI can redeploy and nothing else — no shell,
no file writes, no token reads, even if the deploy key leaks.

## Trust boundary

`deploy` is in the `docker` group, which is root-equivalent **with a shell**.
So we remove the shell: the SSH key is locked to a single forced command
(`command=` in `authorized_keys`). Reachable surface = "run redeploy", nothing more.

## 1. Service user (as root)

```bash
adduser --disabled-password --gecos "" deploy
usermod -aG docker deploy
passwd -l deploy
mkdir -p /opt/fetchly
```

`/opt/fetchly` stays **root-owned**. `deploy` needs no write access anywhere.

## 2. Forced redeploy command (as root)

`/opt/fetchly/deploy.sh` — root-owned, not writable by `deploy`:

```bash
#!/bin/bash
set -euo pipefail
cd /opt/fetchly
docker compose pull
docker compose up -d
docker image prune -f
```

```bash
chown root:root /opt/fetchly/deploy.sh
chmod 755 /opt/fetchly/deploy.sh
```

Mirror of `.github/workflows/deploy.yml` `script:` — if you change one, change
the other. (With the forced command below, the server always runs this file
and ignores whatever command SSH requested.)

## 3. Lock the key to that command (as root)

`~deploy/.ssh/authorized_keys` — **one line**, restrictions first:

```
command="/opt/fetchly/deploy.sh",no-port-forwarding,no-X11-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... fetchly-deploy
```

```bash
chmod 700 ~deploy/.ssh
chmod 600 ~deploy/.ssh/authorized_keys
chown -R deploy:deploy ~deploy/.ssh
```

## 4. Secrets file (as root)

```bash
nano /opt/fetchly/.env   # TELEGRAM_BOT_TOKEN, REDIS_URL=redis://redis:6379, ...
chown root:deploy /opt/fetchly/.env
chmod 640 /opt/fetchly/.env
```

`deploy` can read it (compose needs that) but cannot modify it — the token
cannot be overwritten or exfiltrated through a file write. (Reading via a
shell is impossible: there is no shell.)

## 5. Keypair (on your laptop)

```bash
ssh-keygen -t ed25519 -f ~/.ssh/fetchly_deploy -N "" -C "fetchly-deploy"
# .pub content goes into authorized_keys above; private key never leaves your disk
```

## 6. Test (from your laptop)

```bash
ssh -i ~/.ssh/fetchly_deploy deploy@<SERVER_IP>
# Expected: deploy runs (pull + up -d), prints compose output, disconnects.
# `ssh -t ... bash` must NOT give you a shell — it runs deploy.sh instead.
```

## 7. GitHub secrets

```bash
gh secret set SERVER_HOST --body "<SERVER_IP>"
gh secret set SERVER_USER --body "deploy"
gh secret set SERVER_SSH_KEY < ~/.ssh/fetchly_deploy   # private key, local file → secret, never chat
```

## 8. First boot (manual, once)

```bash
ssh -i ~/.ssh/fetchly_deploy deploy@<SERVER_IP>   # runs deploy; image must exist (push to main first)
```

Check from root on the server:

```bash
docker compose -f /opt/fetchly/docker-compose.yml ps
docker compose -f /opt/fetchly/docker-compose.yml logs -f bot
```

After this, every `main` push self-deploys.

## 9. Firewall + upkeep

```bash
ufw allow 22/tcp && ufw --force enable   # outbound 443 is all the bot needs
```

- Logs: `docker compose -f /opt/fetchly/docker-compose.yml logs -f bot` (as root).
- SQLite backup (cron, as root): `sqlite3 /var/lib/docker/volumes/fetchly_db/_data/fetchly.db ".backup '/root/fetchly-$(date +%F).db'"`. Redis needs no backup (ephemeral).
- Key rotation: replace the one line in `authorized_keys`, update `SERVER_SSH_KEY`.

## What this does NOT protect against

A compromised GitHub Actions runner (or malicious workflow edit on `main`)
can still redeploy a malicious image — forced commands don't sign images.
Mitigations if that ever matters: branch protection + required reviews on
`main`, pin actions by SHA.
