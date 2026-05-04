# JMAP Mail

A clean, minimal webmail client built on [JMAP](https://jmap.io/) (RFC 8620), designed for [Stalwart Mail Server](https://stalw.art/). Inspired by iCloud Mail's three-pane layout with full dark mode support.

![Mail](docs/screenshot-mail.png)

![Compose](docs/screenshot.png)

![Calendar](docs/screenshot-calendar.png)

![Contacts](docs/screenshot-contacts.png)

## Features

- JMAP protocol for fast, efficient mail access (RFC 8620 / RFC 8621)
- **Calendar** — month view, create / edit / delete events, per-calendar filtering (JMAP Calendars / RFC 8984)
- **Contacts** — address book list, contact search, create / edit / delete (JMAP Contacts / RFC 9553)
- OAuth2 authentication via Stalwart's built-in OAuth2 server
- Three-pane layout: mailboxes / message list / reading pane
- Mailbox and Folders sections — system mailboxes (Inbox, Sent, Drafts…) separated from user folders
- App switcher (bottom of the sidebar) to switch between Mail, Calendar, and Contacts
- Dark mode (system preference + manual toggle, persisted)
- Real-time push via JMAP EventSource (SSE)
- Single Docker image — FastAPI serves both the API and the compiled frontend

## Stack

| Layer | Technology |
|---|---|
| Backend | Python 3.12, FastAPI, httpx |
| Frontend | SvelteKit, Tailwind CSS |
| Auth | OAuth2 authorization code flow |
| Protocol | JMAP (RFC 8620, RFC 8621, RFC 8984, RFC 9553) |
| Runtime | Single Docker image (multi-stage build) |

## Quick Start

### Prerequisites

- Docker and Docker Compose
- A running [Stalwart Mail Server](https://stalw.art/) instance with an OAuth2 client configured

### 1. Configure OAuth2 in Stalwart

In the Stalwart admin panel, create an OAuth2 client with:
- **Redirect URI:** `http://localhost:8000/auth/callback` (or your public URL)
- **Grant type:** Authorization Code
- Note the **Client ID** and **Client Secret**

### 2. Configure the application

```bash
cp .env.example .env
```

Edit `.env`:

```env
STALWART_URL=https://mail.example.com
OAUTH_CLIENT_ID=jmap-webclient
OAUTH_CLIENT_SECRET=your-client-secret
OAUTH_REDIRECT_URI=http://localhost:8000/auth/callback
SESSION_SECRET=<output of: python3 -c "import secrets; print(secrets.token_hex(32))">
```

### 3. Run

```bash
docker compose up --build
```

Open `http://localhost:8000` — you will be redirected to Stalwart's OAuth login page.

## Development

Run the backend and frontend separately for a faster dev loop:

**Backend:**
```bash
cd backend
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --reload --port 8000
```

**Frontend** (in a separate terminal):
```bash
cd frontend
npm install
npm run dev
```

The Vite dev server proxies `/api` and `/auth` to `http://localhost:8000`, so both run together at `http://localhost:5173`.

## Docker

The image uses a true multi-stage build:

1. **`frontend-builder`** (Node 20) — runs `npm run build`, producing a static SvelteKit output
2. **`python-deps`** (Python 3.12-slim) — installs Python packages into a prefix directory
3. **Final stage** (Python 3.12-slim) — copies packages and built frontend, runs as a non-root user

```bash
# Build manually
docker build -t jmap-mail .

# Run
docker run -p 8000:8000 --env-file .env jmap-mail
```

## CI/CD (GitLab)

The included `.gitlab-ci.yml` pipeline:

| Stage | Job | What it does |
|---|---|---|
| `build` | `build-image` | Builds and pushes `:sha` + `:branch` tags to the GitLab registry |
| `test` | `smoke-test` | Starts the container and asserts `/health` returns `{"status":"ok"}` |
| `release` | `tag-latest` | Promotes `:sha` to `:latest` — only on `main` |

Uses GitLab's built-in container registry (`$CI_REGISTRY_IMAGE`). No additional variables needed beyond the defaults GitLab injects.

## Project Structure

```
jmap-mail/
├── backend/
│   └── app/
│       ├── main.py       # FastAPI app, routes, static file serving
│       ├── auth.py       # OAuth2 authorization code flow
│       ├── jmap.py       # JMAP session, request proxy, SSE stream
│       └── config.py     # Settings (pydantic-settings)
├── frontend/
│   └── src/
│       ├── lib/
│       │   ├── api.js                   # JMAP API helpers (mail, calendar, contacts)
│       │   ├── stores/
│       │   │   ├── mail.js              # Mail state (mailboxes, emails, session)
│       │   │   ├── calendar.js          # Calendar state (events, view, selection)
│       │   │   └── contacts.js          # Contacts state (address books, search)
│       │   └── components/
│       │       ├── AppNav.svelte         # Bottom app switcher
│       │       ├── CalendarGrid.svelte   # Month-view calendar grid
│       │       ├── EventModal.svelte     # Create / edit calendar event
│       │       ├── ContactModal.svelte   # Create / edit contact
│       │       └── ...                  # Sidebar, MessageList, MessagePane, …
│       └── routes/
│           ├── +layout.svelte            # Auth guard, dark mode bootstrap
│           ├── +page.svelte              # Mail three-pane view
│           ├── calendar/+page.svelte     # Calendar view
│           └── contacts/+page.svelte     # Contacts view
├── Dockerfile
├── docker-compose.yml
├── .env.example
└── .gitlab-ci.yml
```

## Security Notes

- Access tokens are stored server-side in signed session cookies and never sent to the browser
- HTML email bodies are rendered with `{@html}` — add [DOMPurify](https://github.com/cure53/DOMPurify) before deploying to production
- Set `SESSION_SECRET` to a cryptographically random value (see `.env.example`)

## AI Disclosure

This project was scaffolded with the assistance of [Claude](https://claude.ai/) (Anthropic). The architecture, code structure, and implementation were generated through a human-directed conversation and reviewed by the project author. All code should be audited before use in a production environment.

## License

MIT — see [LICENSE](LICENSE).
