"""Faux serveur MCP (stdio) pour les tests : outils « echo », « fail », « token »
(renvoie la variable d'environnement FAKE_TOKEN) et « pid » (numéro du processus)."""

import json
import os
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Renvoie le texte reçu.",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
    },
    {"name": "fail", "description": "Échoue toujours.", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "token", "description": "Renvoie FAKE_TOKEN.", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "pid", "description": "Renvoie le numéro du processus.", "inputSchema": {"type": "object", "properties": {}}},
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
        elif name == "token":
            reply(id_, {"content": [{"type": "text", "text": "jeton : " + os.environ.get("FAKE_TOKEN", "absent")}]})
        elif name == "pid":
            reply(id_, {"content": [{"type": "text", "text": str(os.getpid())}]})
        else:
            reply(id_, {"content": [{"type": "text", "text": "raté"}], "isError": True})
    else:
        reply(id_, error={"code": -32601, "message": "inconnu"})
