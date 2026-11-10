"""Downloads NudeNet's wheel (pinned by SHA-256) and takes the 320n model out of it.
Run while the detector image is built; the URL and hashes come from the Dockerfile."""
import hashlib, os, sys, urllib.request, zipfile
url = os.environ["NUDENET_WHEEL_URL"]
for attempt in range(4):
    try:
        data = urllib.request.urlopen(url, timeout=120).read()
        break
    except Exception as e:
        print(f"download failed ({e}); retrying", file=sys.stderr)
else:
    sys.exit("couldn't download the NudeNet wheel")
if hashlib.sha256(data).hexdigest() != os.environ["NUDENET_WHEEL_SHA256"]:
    sys.exit("the NudeNet wheel's SHA-256 doesn't match")
open("nudenet.whl", "wb").write(data)
z = zipfile.ZipFile("nudenet.whl")
model = z.read("nudenet/320n.onnx")
if hashlib.sha256(model).hexdigest() != os.environ["NUDENET_MODEL_SHA256"]:
    sys.exit("320n.onnx's SHA-256 doesn't match")
os.makedirs("/models", exist_ok=True)
open("/models/320n.onnx", "wb").write(model)
open("/models/LICENSE-nudenet.txt", "wb").write(z.read("nudenet-3.4.2.dist-info/LICENSE"))
print("model ok", len(model), "bytes")
