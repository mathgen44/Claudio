"""Faux serveur MCP (stdio) pour les tests : un outil « echo » et un outil « fail »."""

import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Renvoie le texte reçu.",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
    },
    {"name": "fail", "description": "Échoue toujours.", "inputSchema": {"type": "object", "properties": {}}},
]


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    req = json.loads(line)
    method, id_ = req.get("method"), req.get("id")
    if id_ is None:
        continue  # notification
    if method == "initialize":
        # Le serveur pose aussi une question au client, qui doit y répondre.
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"}) + "\n")
        reply(id_, {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "faux"}})
    elif method == "tools/list":
        reply(id_, {"tools": TOOLS})
    elif method == "tools/call":
        name = req["params"]["name"]
        if name == "echo":
            reply(id_, {"content": [{"type": "text", "text": "écho : " + req["params"]["arguments"]["text"]}]})
        else:
            reply(id_, {"content": [{"type": "text", "text": "raté"}], "isError": True})
    else:
        reply(id_, error={"code": -32601, "message": "inconnu"})
