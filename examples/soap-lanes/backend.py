#!/usr/bin/env python3
"""Stand-in for a REST/JSON flow written for the X-Road REST protocol
(e.g. a Ruuter flow behind a `.guard` on X-Road-Client): flat JSON in,
result wrapped as {"response": "<JSON string>"}, errors as 4xx.
Standard library only. Listens on :18091."""
import http.server, json

class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])) or b"{}")
        client = self.headers.get("x-road-client", "")
        if len([p for p in client.split("/") if p]) != 4:
            status, out = 403, {"error": "FORBIDDEN", "message": "X-Road-Client header is missing or has invalid format"}
        elif len(body.get("personCode", "")) != 11:
            status, out = 400, {"error": "INVALID_PARAMETER", "message": "personCode must be 11 digits"}
        else:
            status, out = 200, {"checks": {"item": [
                {"date": "2026-01-16T09:30:00", "title": "Roadside check"},
                {"date": "2026-01-15T14:00:00", "title": "Café inspection"}]}}
        data = json.dumps({"response": json.dumps(out, ensure_ascii=False)}).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
        print(f"{status} {self.path} x-road-client={client!r} body={json.dumps(body)}", flush=True)

    def log_message(self, *args):
        pass

http.server.ThreadingHTTPServer(("0.0.0.0", 18091), H).serve_forever()
