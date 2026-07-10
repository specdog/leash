from pathlib import Path

from ament_index_python.packages import get_package_share_directory
from launch import LaunchDescription
from launch.actions import IncludeLaunchDescription
from launch.launch_description_sources import PythonLaunchDescriptionSource
from launch_ros.actions import Node


def generate_launch_description():
    package_dir = Path(get_package_share_directory("leash_waveshare_slam"))
    slam_dir = Path(get_package_share_directory("slam_toolbox"))

    bridge = Node(
        package="leash_waveshare_slam",
        executable="leash_ros_bridge",
        name="leash_ros_bridge",
        output="screen",
    )
    ekf = Node(
        package="robot_localization",
        executable="ekf_node",
        name="ekf_filter_node",
        output="screen",
        parameters=[str(package_dir / "config" / "ekf.yaml")],
    )
    slam = IncludeLaunchDescription(
        PythonLaunchDescriptionSource(
            str(slam_dir / "launch" / "online_async_launch.py")
        ),
        launch_arguments={
            "autostart": "true",
            "use_lifecycle_manager": "false",
            "use_sim_time": "false",
            "slam_params_file": str(package_dir / "config" / "slam_toolbox.yaml"),
        }.items(),
    )
    return LaunchDescription([bridge, ekf, slam])
