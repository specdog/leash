FROM ubuntu:22.04

ARG CUVSLAM_WHEEL_URL=https://github.com/nvidia-isaac/cuVSLAM/releases/download/v17.0.0/cuvslam-17.0.0%2Bcu12-cp310-cp310-manylinux_2_35_aarch64.whl
ARG CUVSLAM_WHEEL_SHA256=658250d97c96c5db0d61ed68d8c0e99514c130574339367f0d917ad60f05d315

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m pip install --no-cache-dir \
        numpy==2.2.4 \
        opencv-python-headless==4.11.0.86 \
    && curl -fL "$CUVSLAM_WHEEL_URL" -o /tmp/cuvslam.whl \
    && echo "$CUVSLAM_WHEEL_SHA256  /tmp/cuvslam.whl" | sha256sum -c - \
    && python3 -m pip install --no-cache-dir --no-deps /tmp/cuvslam.whl \
    && rm -f /tmp/cuvslam.whl

RUN useradd --uid 1000 --create-home --shell /usr/sbin/nologin cuvslam

COPY runner.py /opt/leash-cuvslam/runner.py
RUN chmod 0555 /opt/leash-cuvslam/runner.py

USER 1000:1000
WORKDIR /opt/leash-cuvslam

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    LEASH_CUVSLAM_HEALTH_FILE=/run/cuvslam-health.json

HEALTHCHECK --interval=10s --timeout=2s --start-period=30s --retries=3 \
  CMD ["python3", "-c", "import json,time; p=json.load(open('/run/cuvslam-health.json')); assert time.time()*1000-p.get('captured_at_ms',p.get('ts_ms',0))<5000"]

ENTRYPOINT ["python3", "/opt/leash-cuvslam/runner.py"]
