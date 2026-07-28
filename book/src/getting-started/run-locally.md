# Run it locally

**Note**: this scaffold has no shipped functionality yet — the
binary prints a placeholder line and exits. This chapter documents
the intended shape once domain code lands.

## Path A — pull the pre-built image (fastest, once published)

```bash
docker run -d --name xtr -p 8080:8080 \
    turnerrainer/xtr:latest
```

## Path B — build from source

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
docker compose up -d --build
```

First build is 60–90 s. Subsequent starts are seconds.

## Health check (once domain lands)

```bash
curl http://localhost:8080/health
```

Expected response:

```json
{"status":"ok"}
```

## Stop when you're done

```bash
docker rm -f xtr        # Path A
docker compose down     # Path B
```

Next: [Watch the automated tests pass](./automated-tests.md).
