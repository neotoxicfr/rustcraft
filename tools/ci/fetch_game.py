"""Download the official game the way the launcher does, for the CI: version manifest, client jar and its
libraries for this system, each file checked against the SHA-1 Mojang publishes. Nothing is stored in
the repository; the tests then find the game through `RUSTCRAFT_MC_DIR`.

    python tools/ci/fetch_game.py <game dir> [version]
"""
import hashlib, json, os, platform, sys, urllib.request
from concurrent.futures import ThreadPoolExecutor

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
OS = {"Windows": "windows", "Linux": "linux", "Darwin": "osx"}[platform.system()]


def get(url):
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def save(url, sha1, path):
    if os.path.exists(path) and hashlib.sha1(open(path, "rb").read()).hexdigest() == sha1:
        return
    data = get(url)
    if hashlib.sha1(data).hexdigest() != sha1:
        raise SystemExit(f"bad SHA-1 for {url}")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def allowed(rules):
    ok = not rules
    for r in rules or []:
        if r.get("os", {}).get("name") in (None, OS):
            ok = r["action"] == "allow"
    return ok


def main():
    root, version = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "26.3")
    entry = next(v for v in json.loads(get(MANIFEST))["versions"] if v["id"] == version)
    meta_bytes = get(entry["url"])
    if hashlib.sha1(meta_bytes).hexdigest() != entry["sha1"]:
        raise SystemExit("bad SHA-1 for the version file")
    vdir = os.path.join(root, "versions", version)
    os.makedirs(vdir, exist_ok=True)
    with open(os.path.join(vdir, f"{version}.json"), "wb") as f:
        f.write(meta_bytes)
    meta = json.loads(meta_bytes)
    client = meta["downloads"]["client"]
    jobs = [(client["url"], client["sha1"], os.path.join(vdir, f"{version}.jar"))]
    for lib in meta["libraries"]:
        art = lib.get("downloads", {}).get("artifact")
        if art and allowed(lib.get("rules")):
            jobs.append((art["url"], art["sha1"], os.path.join(root, "libraries", art["path"])))
    with ThreadPoolExecutor(8) as pool:
        list(pool.map(lambda j: save(*j), jobs))
    print(f"{version}: client jar and {len(jobs) - 1} libraries in {root}; Java {meta['javaVersion']['majorVersion']}")


if __name__ == "__main__":
    main()
