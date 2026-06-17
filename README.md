# 🍃 leaf

leaf is a Discord bot and gallery for keeping a daily photo/video archive. You
run a Series, post to it regularly, and browse it back as a calendar and day
viewer inside Discord.

It runs as one container: the bot, the web API, the gallery, and an admin panel,
all over a single SQLite database. Media is stored in Cloudflare R2.

## Quick start

```sh
git clone <your-repo-url> leaf && cd leaf
docker compose up -d
docker compose logs -f leaf       # the first run prints a one-time setup code
```

Open `http://localhost:8080/setup`, enter the code, and fill in your Discord and
R2 credentials. leaf validates them and starts.

You'll need a Discord application and a Cloudflare R2 bucket before that step, and
a public HTTPS hostname for anything past local testing. The
[setup guide](guide/README.md) walks through all of it: Discord, Cloudflare
Tunnel and R2, daily use, and importing an old archive.

## Documentation

- [Setup & usage guide](guide/README.md)
- [Local development](guide/06-local-dev.md)
- [Web UI spec](WEB-UI-SPEC.md) (what the setup page and admin panel do)

## License

BSD-3-Clause.
