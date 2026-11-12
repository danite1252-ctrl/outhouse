"""Downloads the detector's models while its image is built, each pinned by
SHA-256 (the URLs and hashes come from the Dockerfile):

- NudeNet's wheel, and the 320n nudity model inside it;
- YuNet (OpenCV Zoo's face detector, 2023mar);
- an age-group classifier (dima806/fairface_age_image_detection, ViT,
  int8 ONNX from onnx-community).

Nothing is downloaded when the detector runs."""
import hashlib, os, sys, urllib.request, zipfile


def fetch(url, sha256, what):
    for attempt in range(4):
        try:
            data = urllib.request.urlopen(url, timeout=300).read()
            break
        except Exception as e:
            print(f"{what}: download failed ({e}); retrying", file=sys.stderr)
    else:
        sys.exit(f"couldn't download {what}")
    if hashlib.sha256(data).hexdigest() != sha256:
        sys.exit(f"{what}: SHA-256 doesn't match")
    print(f"{what} ok", len(data), "bytes")
    return data


os.makedirs("/models", exist_ok=True)

# ---- NudeNet (nudity) ----
wheel = fetch(os.environ["NUDENET_WHEEL_URL"], os.environ["NUDENET_WHEEL_SHA256"], "the NudeNet wheel")
open("nudenet.whl", "wb").write(wheel)
z = zipfile.ZipFile("nudenet.whl")
model = z.read("nudenet/320n.onnx")
if hashlib.sha256(model).hexdigest() != os.environ["NUDENET_MODEL_SHA256"]:
    sys.exit("320n.onnx's SHA-256 doesn't match")
open("/models/320n.onnx", "wb").write(model)
open("/models/LICENSE-nudenet.txt", "wb").write(z.read("nudenet-3.4.2.dist-info/LICENSE"))

# ---- YuNet (faces) ----
open("/models/yunet.onnx", "wb").write(
    fetch(os.environ["YUNET_URL"], os.environ["YUNET_SHA256"], "YuNet"))
open("/models/LICENSE-yunet.txt", "wb").write(
    fetch(os.environ["YUNET_LICENSE_URL"], os.environ["YUNET_LICENSE_SHA256"], "YuNet's licence"))

# ---- Age groups ----
open("/models/age.onnx", "wb").write(
    fetch(os.environ["AGE_MODEL_URL"], os.environ["AGE_MODEL_SHA256"], "the age model"))
open("/models/LICENSE-age.txt", "w").write(
    "Age-group model: dima806/fairface_age_image_detection (ViT-B/16 fine-tuned on\n"
    "the FairFace dataset), int8 ONNX conversion by onnx-community\n"
    "(onnx-community/fairface_age_image_detection-ONNX, model_quantized.onnx).\n"
    "Licensed under the Apache License, Version 2.0 (as stated on the model card):\n"
    "https://www.apache.org/licenses/LICENSE-2.0\n"
    "Source: " + os.environ["AGE_MODEL_URL"] + "\n")
print("models ok")
