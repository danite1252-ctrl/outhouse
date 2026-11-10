# Outhouse image detector (sidecar): NudeNet's 320n model on onnxruntime,
# CPU only. Built by deploy/push.ps1 / deploy/setup.sh (docker compose build)
# and in CI. See deploy/README.md → "Image detector".
#
# The model is downloaded while the image is built (never committed to git):
# NudeNet 3.4.2's wheel from PyPI, checked against its SHA-256, and the
# 320n.onnx inside it checked against its own SHA-256.
FROM python:3.12-slim-bookworm AS model
ARG NUDENET_WHEEL_URL=https://files.pythonhosted.org/packages/1c/ee/1aa02d44ba958cc77e16ff1e41a0aac5e721037db7bf62b9c9d124917f87/nudenet-3.4.2-py3-none-any.whl
ARG NUDENET_WHEEL_SHA256=5937dbd84e5d8e5de038f08ffea5a1bb50a08475776bf2b4795914ce0eaf0331
ARG NUDENET_MODEL_SHA256=c15d8273adad2d0a92f014cc69ab2d6c311a06777a55545f2c4eb46f51911f0f
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
# Build-time self test: the model loads and a plain picture scores low.
RUN python /app/detector.py --check
EXPOSE 8095
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8095/health', timeout=4).status == 200 else 1)"
CMD ["python", "/app/detector.py"]
