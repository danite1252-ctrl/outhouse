# Outhouse image detector (sidecar): NudeNet's 320n nudity model, OpenCV
# Zoo's YuNet face detector and a FairFace age-group classifier, on
# onnxruntime, CPU only. Built by deploy/push.ps1 / deploy/setup.sh (docker
# compose build) and in CI. See deploy/README.md → "Image detector".
#
# The models are downloaded while the image is built (never committed to
# git), each checked against its SHA-256: NudeNet 3.4.2's wheel from PyPI
# (and the 320n.onnx inside it), YuNet 2023mar (MIT) and the age model
# (Apache-2.0), both from Hugging Face at a pinned commit.
FROM python:3.12-slim-bookworm AS model
ARG NUDENET_WHEEL_URL=https://files.pythonhosted.org/packages/1c/ee/1aa02d44ba958cc77e16ff1e41a0aac5e721037db7bf62b9c9d124917f87/nudenet-3.4.2-py3-none-any.whl
ARG NUDENET_WHEEL_SHA256=5937dbd84e5d8e5de038f08ffea5a1bb50a08475776bf2b4795914ce0eaf0331
ARG NUDENET_MODEL_SHA256=c15d8273adad2d0a92f014cc69ab2d6c311a06777a55545f2c4eb46f51911f0f
ARG YUNET_URL=https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx
ARG YUNET_SHA256=8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4
ARG YUNET_LICENSE_URL=https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/LICENSE
ARG YUNET_LICENSE_SHA256=c83b8120c50ccbd4c4f96edf53141bdd566ebb8f8e9227e415326aa1b1aba958
ARG AGE_MODEL_URL=https://huggingface.co/onnx-community/fairface_age_image_detection-ONNX/resolve/fd04cddf04982f6acc1ac76fb1a31b44c9285596/onnx/model_quantized.onnx
ARG AGE_MODEL_SHA256=42633bffee3cb91118f90cc97311270a4f94fa9f8eb7023e277ce045b2b91223
WORKDIR /fetch
COPY fetch_model.py /fetch/fetch_model.py
RUN python /fetch/fetch_model.py

FROM python:3.12-slim-bookworm
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 PIP_DISABLE_PIP_VERSION_CHECK=1
RUN pip install --no-cache-dir onnxruntime==1.31.1 numpy==2.5.4 pillow==13.0.0 \
 && useradd --system --uid 10001 --home /nonexistent --shell /usr/sbin/nologin detector
COPY --from=model /models /models
COPY detector.py /app/detector.py
USER detector
WORKDIR /app
# Build-time self test: the models load, a plain picture scores low with no
# faces, and a drawn face (no real person) is found and gets an age estimate.
RUN python /app/detector.py --check
EXPOSE 8095
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8095/health', timeout=4).status == 200 else 1)"
CMD ["python", "/app/detector.py"]
