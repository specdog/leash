use std::{collections::BTreeMap, env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};

use leash_harness::{
    agent_runtime::{
        AgentConsoleCapability, AgentConsoleHealth, AgentRunOutput, AgentRuntimeSnapshot,
        AgentSession, AgentSessionSummary, AgentTaskRecord, AgentTaskSnapshot, AgentTaskState,
        AgentTaskStopOutput, AgentTurn, CapabilityPermissions,
    },
    calibration::{
        CalibrationEnterRequest, CalibrationEnterResult, CalibrationPhase, CalibrationStatus,
    },
    capability::{CapabilityDescriptor, InvocationOrigin, SafetyClass},
    cognition::{
        CognitionBoundaryFrameV1, CognitionCapabilitiesV1, CognitionCheckpointV1,
        CognitionLayerSnapshotV1, CognitionSnapshotsV1, CognitionStatusV1, SemanticPriorV1,
    },
    daemon::StopOutcome,
    localization::{
        LocalizationProviderSnapshot, LocalizationProviderState, LocalizationProviderStatus,
        LocalizationProviderUpdate,
    },
    mapping::{
        MappingLifecycleAction, MappingLifecycleRequest, MappingLifecycleState, MappingStatus,
    },
    mcp::{
        McpCallResponse, McpModuleToolMap, McpProtocolCallResult, McpProtocolTool,
        McpProtocolToolList, McpStatus, McpTextContent, McpToolDescriptor, McpToolList,
    },
    module::{
        ModuleGraph, ModuleHealth, ModuleInfo, ModuleState, StackBlueprintMetadata,
        StreamDescriptor, StreamDirection,
    },
    navigation::{
        NavigationExecutionStatus, NavigationExecutorKind, NavigationFeedback,
        NavigationGoalRequest, NavigationReadiness, NavigationReadinessQuery,
        NavigationReadinessResponse, NavigationStatusResponse, NavigationTerminalReason,
    },
    replay::{ReplayEvent, ReplayEventKind},
    stack::{AdapterCategory, AdapterMaturity, AdapterProfile},
    transport::NetworkStreamFrame,
    types::{
        AgentMessage, AgentMessageAck, AgentMessageList, AgentModelResponse, AppliedActionEvidence,
        AppliedActionEvidencePage, AutonomyOverlay, BatteryStatus, CameraRecoveryResponse,
        CameraStatus, CameraStreamFailure, CameraStreamHealth, Capabilities, CaptureResult,
        CommandOverlay, CommandStreamState, CostmapFrame, DetectionFrame, DriveOutcome,
        DroneCommandStatus, Health, ImageObservation, ImuSample, ImuStatus, LocalizationFrame,
        LocalizationHealth, LocalizationStatus, ManipulatorCommandStatus, ManipulatorJoint,
        ManipulatorJointState, MapIdentity, MapMetadata, MotionEvent, MotionEventKind,
        OccupancyGridFrame, OdometryStatus, OperatorSessionEvent, OperatorSessionEventKind,
        OperatorSessionRecording, OperatorSessionRobot, OperatorTokenStatus, PatrolStatus,
        PatrolStrategy, PatrolZone, PatrolZoneList, PlanarRangeScan, PlannerGoal, PlannerStatus,
        PointCloudMetadata, Pose2d, PoseWithCovariance2d, Quaternion, RangeScanStatus,
        RawFrameStatus, ResourceSample, RunLogEntry, SafetyStreamState, SavedWaypoint,
        SavedWaypointList, SensorDataStatus, SensorSnapshot, SpatialMemoryEntry, SpatialMemoryKind,
        SpatialMemoryStatus, SpeedMode, TelemetryFrame, TelemetryStreamFrame, Twist2d, Vector3Si,
        VerifiedZeroEvidence, VisionResult, VisualizationFrame, VisualizationPath,
        ZeroCommandReason, ZoneBoundaryPoint,
    },
    worker::{
        ExternalWorkerState, ExternalWorkerStatus, WorkerInputFrame, WorkerInputPayload,
        WorkerOutputFrame, WorkerOutputPayload,
    },
};

const DEFAULT_OUTPUT: &str = "schemas/leash-messages.schema.json";
const SCHEMA_VERSION: &str = "leash-message-schema-v1";
const SCHEMA_ID: &str = "https://specdog.github.io/leash/schemas/leash-messages.schema.json";

#[derive(Debug)]
struct Args {
    output: PathBuf,
    check: bool,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let rendered = format!("{}\n", serde_json::to_string_pretty(&schema_document()?)?);

    if args.check {
        let current = fs::read_to_string(&args.output)
            .with_context(|| format!("read {}", args.output.display()))?;
        if normalize_newlines(&current) != rendered {
            eprintln!(
                "{} is stale; run `cargo run --features mcp --bin leash-schema -- --output {}`",
                args.output.display(),
                args.output.display()
            );
            process::exit(1);
        }
        println!("schema current: {}", args.output.display());
        return Ok(());
    }

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&args.output, rendered)
        .with_context(|| format!("write {}", args.output.display()))?;
    println!("wrote {}", args.output.display());
    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut output = PathBuf::from(DEFAULT_OUTPUT);
    let mut check = false;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--output" | "-o" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("{arg} requires a path argument"))?;
                output = PathBuf::from(value);
            }
            "--help" | "-h" => {
                println!("Usage: leash-schema [--check] [--output PATH]");
                process::exit(0);
            }
            other => return Err(anyhow!("unknown argument '{other}'")),
        }
    }

    Ok(Args { output, check })
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn schema_document() -> Result<Value> {
    let mut schemas = BTreeMap::new();

    insert::<Health>(&mut schemas, "Health")?;
    insert::<OperatorTokenStatus>(&mut schemas, "OperatorTokenStatus")?;
    insert::<Capabilities>(&mut schemas, "Capabilities")?;
    insert::<TelemetryFrame>(&mut schemas, "TelemetryFrame")?;
    insert::<TelemetryStreamFrame>(&mut schemas, "TelemetryStreamFrame")?;
    insert::<CommandStreamState>(&mut schemas, "CommandStreamState")?;
    insert::<SafetyStreamState>(&mut schemas, "SafetyStreamState")?;
    insert::<SensorSnapshot>(&mut schemas, "SensorSnapshot")?;
    insert::<BatteryStatus>(&mut schemas, "BatteryStatus")?;
    insert::<OdometryStatus>(&mut schemas, "OdometryStatus")?;
    insert::<CameraStatus>(&mut schemas, "CameraStatus")?;
    insert::<CameraStreamFailure>(&mut schemas, "CameraStreamFailure")?;
    insert::<CameraStreamHealth>(&mut schemas, "CameraStreamHealth")?;
    insert::<CameraRecoveryResponse>(&mut schemas, "CameraRecoveryResponse")?;
    insert::<RawFrameStatus>(&mut schemas, "RawFrameStatus")?;
    insert::<SensorDataStatus>(&mut schemas, "SensorDataStatus")?;
    insert::<PlanarRangeScan>(&mut schemas, "PlanarRangeScan")?;
    insert::<RangeScanStatus>(&mut schemas, "RangeScanStatus")?;
    insert::<Vector3Si>(&mut schemas, "Vector3Si")?;
    insert::<Quaternion>(&mut schemas, "Quaternion")?;
    insert::<ImuSample>(&mut schemas, "ImuSample")?;
    insert::<ImuStatus>(&mut schemas, "ImuStatus")?;
    insert::<MapIdentity>(&mut schemas, "MapIdentity")?;
    insert::<PoseWithCovariance2d>(&mut schemas, "PoseWithCovariance2d")?;
    insert::<LocalizationStatus>(&mut schemas, "LocalizationStatus")?;
    insert::<LocalizationHealth>(&mut schemas, "LocalizationHealth")?;
    insert::<LocalizationFrame>(&mut schemas, "LocalizationFrame")?;
    insert::<LocalizationProviderState>(&mut schemas, "LocalizationProviderState")?;
    insert::<LocalizationProviderStatus>(&mut schemas, "LocalizationProviderStatus")?;
    insert::<LocalizationProviderUpdate>(&mut schemas, "LocalizationProviderUpdate")?;
    insert::<LocalizationProviderSnapshot>(&mut schemas, "LocalizationProviderSnapshot")?;
    insert::<MappingLifecycleAction>(&mut schemas, "MappingLifecycleAction")?;
    insert::<MappingLifecycleRequest>(&mut schemas, "MappingLifecycleRequest")?;
    insert::<MappingLifecycleState>(&mut schemas, "MappingLifecycleState")?;
    insert::<MappingStatus>(&mut schemas, "MappingStatus")?;
    insert::<CalibrationPhase>(&mut schemas, "CalibrationPhase")?;
    insert::<CalibrationEnterRequest>(&mut schemas, "CalibrationEnterRequest")?;
    insert::<CalibrationEnterResult>(&mut schemas, "CalibrationEnterResult")?;
    insert::<CalibrationStatus>(&mut schemas, "CalibrationStatus")?;
    insert::<SpeedMode>(&mut schemas, "SpeedMode")?;
    insert::<ResourceSample>(&mut schemas, "ResourceSample")?;
    insert::<RunLogEntry>(&mut schemas, "RunLogEntry")?;
    insert::<AgentMessage>(&mut schemas, "AgentMessage")?;
    insert::<AgentMessageAck>(&mut schemas, "AgentMessageAck")?;
    insert::<AgentMessageList>(&mut schemas, "AgentMessageList")?;
    insert::<AgentModelResponse>(&mut schemas, "AgentModelResponse")?;
    insert::<AgentTurn>(&mut schemas, "AgentTurn")?;
    insert::<AgentSession>(&mut schemas, "AgentSession")?;
    insert::<AgentSessionSummary>(&mut schemas, "AgentSessionSummary")?;
    insert::<AgentRunOutput>(&mut schemas, "AgentRunOutput")?;
    insert::<AgentConsoleHealth>(&mut schemas, "AgentConsoleHealth")?;
    insert::<AgentConsoleCapability>(&mut schemas, "AgentConsoleCapability")?;
    insert::<AgentTaskSnapshot>(&mut schemas, "AgentTaskSnapshot")?;
    insert::<AgentRuntimeSnapshot>(&mut schemas, "AgentRuntimeSnapshot")?;
    insert::<CapabilityPermissions>(&mut schemas, "CapabilityPermissions")?;
    insert::<AgentTaskState>(&mut schemas, "AgentTaskState")?;
    insert::<AgentTaskRecord>(&mut schemas, "AgentTaskRecord")?;
    insert::<AgentTaskStopOutput>(&mut schemas, "AgentTaskStopOutput")?;
    insert::<StopOutcome>(&mut schemas, "StopOutcome")?;
    insert::<AppliedActionEvidence>(&mut schemas, "AppliedActionEvidence")?;
    insert::<AppliedActionEvidencePage>(&mut schemas, "AppliedActionEvidencePage")?;
    insert::<CognitionCapabilitiesV1>(&mut schemas, "CognitionCapabilitiesV1")?;
    insert::<CognitionLayerSnapshotV1>(&mut schemas, "CognitionLayerSnapshotV1")?;
    insert::<CognitionSnapshotsV1>(&mut schemas, "CognitionSnapshotsV1")?;
    insert::<CognitionBoundaryFrameV1>(&mut schemas, "CognitionBoundaryFrameV1")?;
    insert::<SemanticPriorV1>(&mut schemas, "SemanticPriorV1")?;
    insert::<CognitionCheckpointV1>(&mut schemas, "CognitionCheckpointV1")?;
    insert::<CognitionStatusV1>(&mut schemas, "CognitionStatusV1")?;
    insert::<ModuleGraph>(&mut schemas, "ModuleGraph")?;
    insert::<StackBlueprintMetadata>(&mut schemas, "StackBlueprintMetadata")?;
    insert::<ModuleInfo>(&mut schemas, "ModuleInfo")?;
    insert::<ModuleHealth>(&mut schemas, "ModuleHealth")?;
    insert::<ModuleState>(&mut schemas, "ModuleState")?;
    insert::<StreamDescriptor>(&mut schemas, "StreamDescriptor")?;
    insert::<StreamDirection>(&mut schemas, "StreamDirection")?;
    insert::<CapabilityDescriptor>(&mut schemas, "CapabilityDescriptor")?;
    insert::<SafetyClass>(&mut schemas, "SafetyClass")?;
    insert::<InvocationOrigin>(&mut schemas, "InvocationOrigin")?;
    insert::<CaptureResult>(&mut schemas, "CaptureResult")?;
    insert::<DriveOutcome>(&mut schemas, "DriveOutcome")?;
    insert::<ZeroCommandReason>(&mut schemas, "ZeroCommandReason")?;
    insert::<VerifiedZeroEvidence>(&mut schemas, "VerifiedZeroEvidence")?;
    insert::<PlannerGoal>(&mut schemas, "PlannerGoal")?;
    insert::<PlannerStatus>(&mut schemas, "PlannerStatus")?;
    insert::<NavigationExecutorKind>(&mut schemas, "NavigationExecutorKind")?;
    insert::<NavigationReadiness>(&mut schemas, "NavigationReadiness")?;
    insert::<NavigationReadinessQuery>(&mut schemas, "NavigationReadinessQuery")?;
    insert::<NavigationReadinessResponse>(&mut schemas, "NavigationReadinessResponse")?;
    insert::<NavigationTerminalReason>(&mut schemas, "NavigationTerminalReason")?;
    insert::<NavigationFeedback>(&mut schemas, "NavigationFeedback")?;
    insert::<NavigationExecutionStatus>(&mut schemas, "NavigationExecutionStatus")?;
    insert::<NavigationGoalRequest>(&mut schemas, "NavigationGoalRequest")?;
    insert::<NavigationStatusResponse>(&mut schemas, "NavigationStatusResponse")?;
    insert::<PatrolStrategy>(&mut schemas, "PatrolStrategy")?;
    insert::<PatrolStatus>(&mut schemas, "PatrolStatus")?;
    insert::<SavedWaypoint>(&mut schemas, "SavedWaypoint")?;
    insert::<SavedWaypointList>(&mut schemas, "SavedWaypointList")?;
    insert::<ZoneBoundaryPoint>(&mut schemas, "ZoneBoundaryPoint")?;
    insert::<PatrolZone>(&mut schemas, "PatrolZone")?;
    insert::<PatrolZoneList>(&mut schemas, "PatrolZoneList")?;
    insert::<MotionEventKind>(&mut schemas, "MotionEventKind")?;
    insert::<MotionEvent>(&mut schemas, "MotionEvent")?;
    insert::<OperatorSessionEventKind>(&mut schemas, "OperatorSessionEventKind")?;
    insert::<OperatorSessionRobot>(&mut schemas, "OperatorSessionRobot")?;
    insert::<OperatorSessionEvent>(&mut schemas, "OperatorSessionEvent")?;
    insert::<OperatorSessionRecording>(&mut schemas, "OperatorSessionRecording")?;
    insert::<ReplayEventKind>(&mut schemas, "ReplayEventKind")?;
    insert::<ReplayEvent>(&mut schemas, "ReplayEvent")?;
    insert::<SpatialMemoryKind>(&mut schemas, "SpatialMemoryKind")?;
    insert::<SpatialMemoryEntry>(&mut schemas, "SpatialMemoryEntry")?;
    insert::<SpatialMemoryStatus>(&mut schemas, "SpatialMemoryStatus")?;
    insert::<VisualizationFrame>(&mut schemas, "VisualizationFrame")?;
    insert::<VisualizationPath>(&mut schemas, "VisualizationPath")?;
    insert::<MapMetadata>(&mut schemas, "MapMetadata")?;
    insert::<Pose2d>(&mut schemas, "Pose2d")?;
    insert::<Twist2d>(&mut schemas, "Twist2d")?;
    insert::<OccupancyGridFrame>(&mut schemas, "OccupancyGridFrame")?;
    insert::<CostmapFrame>(&mut schemas, "CostmapFrame")?;
    insert::<PointCloudMetadata>(&mut schemas, "PointCloudMetadata")?;
    insert::<DetectionFrame>(&mut schemas, "DetectionFrame")?;
    insert::<CommandOverlay>(&mut schemas, "CommandOverlay")?;
    insert::<AutonomyOverlay>(&mut schemas, "AutonomyOverlay")?;
    insert::<ImageObservation>(&mut schemas, "ImageObservation")?;
    insert::<VisionResult>(&mut schemas, "VisionResult")?;
    insert::<WorkerInputFrame>(&mut schemas, "WorkerInputFrame")?;
    insert::<WorkerInputPayload>(&mut schemas, "WorkerInputPayload")?;
    insert::<WorkerOutputFrame>(&mut schemas, "WorkerOutputFrame")?;
    insert::<WorkerOutputPayload>(&mut schemas, "WorkerOutputPayload")?;
    insert::<ExternalWorkerState>(&mut schemas, "ExternalWorkerState")?;
    insert::<ExternalWorkerStatus>(&mut schemas, "ExternalWorkerStatus")?;
    insert::<AdapterCategory>(&mut schemas, "AdapterCategory")?;
    insert::<AdapterMaturity>(&mut schemas, "AdapterMaturity")?;
    insert::<AdapterProfile>(&mut schemas, "AdapterProfile")?;
    insert::<NetworkStreamFrame>(&mut schemas, "NetworkStreamFrame")?;
    insert::<McpToolDescriptor>(&mut schemas, "McpToolDescriptor")?;
    insert::<McpToolList>(&mut schemas, "McpToolList")?;
    insert::<McpCallResponse>(&mut schemas, "McpCallResponse")?;
    insert::<McpStatus>(&mut schemas, "McpStatus")?;
    insert::<McpModuleToolMap>(&mut schemas, "McpModuleToolMap")?;
    insert::<McpProtocolTool>(&mut schemas, "McpProtocolTool")?;
    insert::<McpProtocolToolList>(&mut schemas, "McpProtocolToolList")?;
    insert::<McpTextContent>(&mut schemas, "McpTextContent")?;
    insert::<McpProtocolCallResult>(&mut schemas, "McpProtocolCallResult")?;
    insert::<DroneCommandStatus>(&mut schemas, "DroneCommandStatus")?;
    insert::<ManipulatorJoint>(&mut schemas, "ManipulatorJoint")?;
    insert::<ManipulatorJointState>(&mut schemas, "ManipulatorJointState")?;
    insert::<ManipulatorCommandStatus>(&mut schemas, "ManipulatorCommandStatus")?;

    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SCHEMA_ID,
        "title": "Leash message schemas",
        "description": "Canonical JSON Schema output for Leash HTTP, MCP, telemetry, capability, module, and adapter messages generated from Rust types.",
        "schema_version": SCHEMA_VERSION,
        "schemas": schemas,
    }))
}

fn insert<T: JsonSchema>(schemas: &mut BTreeMap<String, Value>, name: &str) -> Result<()> {
    schemas.insert(name.to_string(), serde_json::to_value(schema_for!(T))?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_newlines;

    #[test]
    fn schema_freshness_ignores_checkout_line_endings() {
        assert_eq!(
            normalize_newlines("{\r\n  \"type\": \"object\"\r\n}\r\n"),
            "{\n  \"type\": \"object\"\n}\n"
        );
    }
}
