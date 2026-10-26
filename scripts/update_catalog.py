"""Régénère catalog/models.json à partir de catalog/sources.json et de l'API Hugging Face.

Usage : python -I scripts/update_catalog.py

sources.json décrit les modèles retenus (dépôt, qualité estimée, cache KV…) ;
ce script y ajoute, pour chaque quantification demandée, le nom exact du fichier
et sa taille lus sur Hugging Face.
"""

import json
import pathlib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCES = ROOT / "catalog" / "sources.json"
OUTPUT = ROOT / "catalog" / "models.json"


def tree(repo: str) -> list[dict]:
    url = f"https://huggingface.co/api/models/{repo}/tree/main"
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.load(r)


def main() -> None:
    sources = json.loads(SOURCES.read_text(encoding="utf-8"))
    models = []
    for m in sources["models"]:
        files = {f["path"]: f for f in tree(m["repo"]) if f["path"].endswith(".gguf")}
        quants = []
        for q in m.pop("quants"):
            match = [p for p in files if p.endswith(f"-{q}.gguf") and not p.startswith("mmproj")]
            if not match:
                print(f"  ! {m['repo']} : quantification {q} introuvable")
                continue
            path = min(match, key=len)
            quants.append({"name": q, "file": path, "size": files[path]["size"]})
        m["quants"] = quants
        models.append(m)
        print(f"{m['id']}: {len(quants)} quantifications")
    out = {"version": sources["version"], "models": models}
    OUTPUT.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
