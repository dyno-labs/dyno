#!/bin/sh
# A browser driver for tests: it reads the run request and sends a short run that went idle.
read -r request
cat <<'END'
{"type":"hello","protocol_version":0,"driver":{"name":"fake","version":"1","browser":null}}
{"type":"mark","mark":"navigated","t_ms":0.0}
{"type":"probe","event":{"type":"first_frame","t_ms":120.0}}
{"type":"mark","mark":"idle","t_ms":900.0}
{"type":"done","t_ms":1000.0}
END
