FROM ros:humble-ros-base-jammy@sha256:9bdda47f584f33aae18456225a8a95fe7bcde821727757f02a3252cbc46e8188

ARG ROBOT_LOCALIZATION_VERSION=3.5.4-1jammy.20260605.164056
ARG SLAM_TOOLBOX_VERSION=2.6.10-1jammy.20260613.004837
ARG TF2_ROS_VERSION=0.25.20-2jammy.20260605.160702
ARG TF2_MSGS_VERSION=0.25.20-2jammy.20260605.135146
ARG ROS2LAUNCH_VERSION=0.19.13-1jammy.20260607.091436
ARG LAUNCH_ROS_VERSION=0.19.13-1jammy.20260605.144337
ARG NAV_MSGS_VERSION=4.9.1-1jammy.20260605.124244
ARG SENSOR_MSGS_VERSION=4.9.1-1jammy.20260605.124600
ARG GEOMETRY_MSGS_VERSION=4.9.1-1jammy.20260605.122512

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

COPY packages.lock /tmp/leash-ros-packages.lock

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        "ros-humble-robot-localization=${ROBOT_LOCALIZATION_VERSION}" \
        "ros-humble-slam-toolbox=${SLAM_TOOLBOX_VERSION}" \
        "ros-humble-tf2-ros=${TF2_ROS_VERSION}" \
        "ros-humble-tf2-msgs=${TF2_MSGS_VERSION}" \
        "ros-humble-ros2launch=${ROS2LAUNCH_VERSION}" \
        "ros-humble-launch-ros=${LAUNCH_ROS_VERSION}" \
        "ros-humble-nav-msgs=${NAV_MSGS_VERSION}" \
        "ros-humble-sensor-msgs=${SENSOR_MSGS_VERSION}" \
        "ros-humble-geometry-msgs=${GEOMETRY_MSGS_VERSION}" \
    && while IFS='=' read -r package expected; do \
         actual="$(dpkg-query -W -f='${Version}' "$package")"; \
         [[ "$actual" == "$expected" ]] || { \
           echo "package lock mismatch: $package expected $expected got $actual" >&2; \
           exit 1; \
         }; \
       done < /tmp/leash-ros-packages.lock \
    && rm -f /tmp/leash-ros-packages.lock \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 1000 leash-ros \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash leash-ros

COPY package.xml /opt/ros/humble/share/leash_waveshare_slam/package.xml
COPY resource/leash_waveshare_slam /opt/ros/humble/share/ament_index/resource_index/packages/leash_waveshare_slam
COPY launch /opt/ros/humble/share/leash_waveshare_slam/launch
COPY config /opt/ros/humble/share/leash_waveshare_slam/config
COPY leash_waveshare_slam /opt/ros/humble/lib/python3.10/site-packages/leash_waveshare_slam
COPY bin/leash_ros_bridge /opt/ros/humble/lib/leash_waveshare_slam/leash_ros_bridge
COPY bin/container_health.sh /usr/local/bin/leash-ros-health

RUN chmod 0755 \
        /opt/ros/humble/lib/leash_waveshare_slam/leash_ros_bridge \
        /usr/local/bin/leash-ros-health \
    && mkdir -p /data/maps \
    && chown -R 1000:1000 /data /home/leash-ros

USER 1000:1000
WORKDIR /home/leash-ros

ENV HOME=/tmp/leash-ros \
    PYTHONUNBUFFERED=1 \
    ROS_DISTRO=humble \
    ROS_HOME=/tmp/leash-ros/.ros \
    ROS_LOG_DIR=/tmp/leash-ros/.ros/log

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=3 \
  CMD ["/usr/local/bin/leash-ros-health"]

ENTRYPOINT ["/ros_entrypoint.sh"]
CMD ["ros2", "launch", "leash_waveshare_slam", "bringup.launch.py"]
