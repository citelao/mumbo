# Mumble server

A simple compose file for running a local Docker script.

## Usage

```bash
# In root dir
npm run mumble:up
npm run mumble:down
# follow logs:
npm run mumble:logs

# Or here
# maybe: podman machine init
# maybe: podman machine start
podman compose up -d
podman compose down
```

### Connect to it

* Server: `localhost`
* Port: `64738`
* Password: `test`
* Name: (whatever)