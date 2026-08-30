use std::{fmt, sync::Arc};

use cudarc::{
    driver::{
        CudaContext, CudaFunction, CudaSlice, CudaStream, DeviceRepr, LaunchConfig, PushKernelArg,
        ValidAsZeroBits,
    },
    nvrtc::Ptx,
};

use crate::{
    executor::{
        validate_cognition_checkpoint, Backend, CognitionLayerMetrics, CognitionLayerSnapshot,
        CognitionStep, ComputeJob, ComputeResult, PredictiveState, ResidentCognitionCheckpoint,
        ResidentCognitionLayer, WorkError,
    },
    CollisionSector, ComputeInputError, LidarPoint, SpatialScan, PREBUILT_FATBIN,
};

#[derive(Debug)]
pub(crate) struct DeviceError(String);

struct CudaCognitionLayer {
    activation: CudaSlice<f32>,
    weights: CudaSlice<f32>,
    bias: CudaSlice<f32>,
    sequence: u64,
    precision: f32,
    prediction_error_l2: f32,
    activation_mean: f32,
    activation_rms: f32,
}

struct CudaCognitionState {
    dimension: usize,
    sensor: CudaSlice<f32>,
    top_down: CudaSlice<f32>,
    layers: Vec<CudaCognitionLayer>,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn device_error(context: &'static str, error: impl fmt::Display) -> DeviceError {
    DeviceError(format!("{context}: {error}"))
}

pub(crate) struct CudaBackend {
    stream: Arc<CudaStream>,
    project_occupancy: CudaFunction,
    lidar_transform: CudaFunction,
    spatial_window_transform: CudaFunction,
    collision_sector_reduce: CudaFunction,
    normalize_rgb: CudaFunction,
    predictive_step: CudaFunction,
    predictive_step_metrics: CudaFunction,
    occupancy_cells: CudaSlice<i8>,
    occupancy_output: CudaSlice<i32>,
    lidar_ranges: CudaSlice<f32>,
    lidar_x: CudaSlice<f32>,
    lidar_y: CudaSlice<f32>,
    lidar_valid: CudaSlice<u8>,
    spatial_scan_indices: CudaSlice<u32>,
    spatial_local_indices: CudaSlice<u32>,
    spatial_angle_min: CudaSlice<f32>,
    spatial_angle_increment: CudaSlice<f32>,
    spatial_range_min: CudaSlice<f32>,
    spatial_range_max: CudaSlice<f32>,
    spatial_clockwise: CudaSlice<i32>,
    spatial_pose_x: CudaSlice<f32>,
    spatial_pose_y: CudaSlice<f32>,
    spatial_pose_yaw: CudaSlice<f32>,
    collision_minimum_bits: CudaSlice<u32>,
    collision_sample_count: CudaSlice<u32>,
    rgb_input: CudaSlice<u8>,
    rgb_output: CudaSlice<f32>,
    predictive_lower: CudaSlice<f32>,
    predictive_state: CudaSlice<f32>,
    predictive_top_down: CudaSlice<f32>,
    predictive_weights: CudaSlice<f32>,
    predictive_bias: CudaSlice<f32>,
    cognition_reductions: CudaSlice<f32>,
    cognition: Option<CudaCognitionState>,
}

impl CudaBackend {
    pub(crate) fn new() -> Result<Self, DeviceError> {
        let device_count = CudaContext::device_count()
            .map_err(|error| device_error("query CUDA device count", error))?;
        if device_count == 0 {
            return Err(DeviceError("no CUDA device is available".to_string()));
        }
        let context = CudaContext::new(0)
            .map_err(|error| device_error("create CUDA device 0 context", error))?;
        let stream = context.default_stream();
        let module = context
            .load_module(Ptx::from_binary(PREBUILT_FATBIN.to_vec()))
            .map_err(|error| device_error("load prebuilt CUDA module", error))?;
        let project_occupancy = module
            .load_function("project_occupancy")
            .map_err(|error| device_error("load project_occupancy", error))?;
        let lidar_transform = module
            .load_function("lidar_transform")
            .map_err(|error| device_error("load lidar_transform", error))?;
        let spatial_window_transform = module
            .load_function("spatial_window_transform")
            .map_err(|error| device_error("load spatial_window_transform", error))?;
        let collision_sector_reduce = module
            .load_function("collision_sector_reduce")
            .map_err(|error| device_error("load collision_sector_reduce", error))?;
        let normalize_rgb = module
            .load_function("normalize_rgb_u8")
            .map_err(|error| device_error("load normalize_rgb_u8", error))?;
        let predictive_step = module
            .load_function("predictive_step")
            .map_err(|error| device_error("load predictive_step", error))?;
        let predictive_step_metrics = module
            .load_function("predictive_step_metrics")
            .map_err(|error| device_error("load predictive_step_metrics", error))?;

        Ok(Self {
            occupancy_cells: allocate_one(&stream, "occupancy cells")?,
            occupancy_output: allocate_one(&stream, "occupancy output")?,
            lidar_ranges: allocate_one(&stream, "lidar ranges")?,
            lidar_x: allocate_one(&stream, "lidar x")?,
            lidar_y: allocate_one(&stream, "lidar y")?,
            lidar_valid: allocate_one(&stream, "lidar validity")?,
            spatial_scan_indices: allocate_one(&stream, "spatial scan indices")?,
            spatial_local_indices: allocate_one(&stream, "spatial local indices")?,
            spatial_angle_min: allocate_one(&stream, "spatial angle minimums")?,
            spatial_angle_increment: allocate_one(&stream, "spatial angle increments")?,
            spatial_range_min: allocate_one(&stream, "spatial range minimums")?,
            spatial_range_max: allocate_one(&stream, "spatial range maximums")?,
            spatial_clockwise: allocate_one(&stream, "spatial scan directions")?,
            spatial_pose_x: allocate_one(&stream, "spatial pose x")?,
            spatial_pose_y: allocate_one(&stream, "spatial pose y")?,
            spatial_pose_yaw: allocate_one(&stream, "spatial pose yaw")?,
            collision_minimum_bits: allocate_one(&stream, "collision minimum")?,
            collision_sample_count: allocate_one(&stream, "collision sample count")?,
            rgb_input: allocate_one(&stream, "RGB input")?,
            rgb_output: allocate_one(&stream, "RGB output")?,
            predictive_lower: allocate_one(&stream, "predictive lower")?,
            predictive_state: allocate_one(&stream, "predictive state")?,
            predictive_top_down: allocate_one(&stream, "predictive top down")?,
            predictive_weights: allocate_one(&stream, "predictive weights")?,
            predictive_bias: allocate_one(&stream, "predictive bias")?,
            cognition_reductions: stream
                .alloc_zeros(3)
                .map_err(|error| device_error("cognition reductions", error))?,
            cognition: None,
            stream,
            project_occupancy,
            lidar_transform,
            spatial_window_transform,
            collision_sector_reduce,
            normalize_rgb,
            predictive_step,
            predictive_step_metrics,
        })
    }

    fn project(&mut self, cells: &[i8], depth: u32) -> Result<Vec<i32>, WorkError> {
        if depth == 0 {
            return Err(ComputeInputError::ZeroDepth.into());
        }
        let output_count = cells
            .len()
            .checked_mul(depth as usize)
            .ok_or(ComputeInputError::LengthOverflow)?;
        if output_count == 0 {
            return Ok(Vec::new());
        }
        let cell_count = u32::try_from(cells.len())
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        let launch_count = u32::try_from(output_count)
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        ensure_capacity(&self.stream, &mut self.occupancy_cells, cells.len())?;
        ensure_capacity(&self.stream, &mut self.occupancy_output, output_count)?;
        self.stream
            .memcpy_htod(cells, &mut self.occupancy_cells)
            .map_err(|error| backend_error("upload occupancy cells", error))?;
        {
            let cells_view = self.occupancy_cells.slice(..cells.len());
            let mut output_view = self.occupancy_output.slice_mut(..output_count);
            unsafe {
                self.stream
                    .launch_builder(&self.project_occupancy)
                    .arg(&cells_view)
                    .arg(&mut output_view)
                    .arg(&cell_count)
                    .arg(&depth)
                    .launch(LaunchConfig::for_num_elems(launch_count))
            }
            .map_err(|error| backend_error("launch project_occupancy", error))?;
        }
        let output_view = self.occupancy_output.slice(..output_count);
        self.stream
            .clone_dtoh(&output_view)
            .map_err(|error| backend_error("download occupancy output", error))
    }

    #[allow(clippy::too_many_arguments)]
    fn lidar(
        &mut self,
        ranges_m: &[f32],
        angle_min_rad: f32,
        angle_increment_rad: f32,
        range_min_m: f32,
        range_max_m: f32,
        yaw_offset_rad: f32,
        clockwise: bool,
        upload_ranges: bool,
    ) -> Result<Vec<LidarPoint>, WorkError> {
        if !range_min_m.is_finite()
            || !range_max_m.is_finite()
            || range_min_m < 0.0
            || range_max_m < range_min_m
        {
            return Err(ComputeInputError::InvalidRangeBounds.into());
        }
        if !angle_min_rad.is_finite()
            || !angle_increment_rad.is_finite()
            || !yaw_offset_rad.is_finite()
        {
            return Err(ComputeInputError::InvalidAngles.into());
        }
        if ranges_m.is_empty() {
            return Ok(Vec::new());
        }
        let count = u32::try_from(ranges_m.len())
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        ensure_capacity(&self.stream, &mut self.lidar_ranges, ranges_m.len())?;
        ensure_capacity(&self.stream, &mut self.lidar_x, ranges_m.len())?;
        ensure_capacity(&self.stream, &mut self.lidar_y, ranges_m.len())?;
        ensure_capacity(&self.stream, &mut self.lidar_valid, ranges_m.len())?;
        if upload_ranges {
            self.stream
                .memcpy_htod(ranges_m, &mut self.lidar_ranges)
                .map_err(|error| backend_error("upload lidar ranges", error))?;
        }
        let clockwise = i32::from(clockwise);
        {
            let ranges = self.lidar_ranges.slice(..ranges_m.len());
            let mut x = self.lidar_x.slice_mut(..ranges_m.len());
            let mut y = self.lidar_y.slice_mut(..ranges_m.len());
            let mut valid = self.lidar_valid.slice_mut(..ranges_m.len());
            unsafe {
                self.stream
                    .launch_builder(&self.lidar_transform)
                    .arg(&ranges)
                    .arg(&mut x)
                    .arg(&mut y)
                    .arg(&mut valid)
                    .arg(&count)
                    .arg(&angle_min_rad)
                    .arg(&angle_increment_rad)
                    .arg(&range_min_m)
                    .arg(&range_max_m)
                    .arg(&yaw_offset_rad)
                    .arg(&clockwise)
                    .launch(LaunchConfig::for_num_elems(count))
            }
            .map_err(|error| backend_error("launch lidar_transform", error))?;
        }
        let x = self
            .stream
            .clone_dtoh(&self.lidar_x.slice(..ranges_m.len()))
            .map_err(|error| backend_error("download lidar x", error))?;
        let y = self
            .stream
            .clone_dtoh(&self.lidar_y.slice(..ranges_m.len()))
            .map_err(|error| backend_error("download lidar y", error))?;
        let valid = self
            .stream
            .clone_dtoh(&self.lidar_valid.slice(..ranges_m.len()))
            .map_err(|error| backend_error("download lidar validity", error))?;
        Ok(x.into_iter()
            .zip(y)
            .zip(valid)
            .map(|((x_m, y_m), valid)| LidarPoint {
                x_m,
                y_m,
                valid: valid != 0,
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn collision_sector(
        &mut self,
        ranges_m: &[f32],
        angle_min_rad: f32,
        angle_increment_rad: f32,
        range_min_m: f32,
        range_max_m: f32,
        sector_center_rad: f32,
        sector_half_width_rad: f32,
        upload_ranges: bool,
    ) -> Result<CollisionSector, WorkError> {
        if !range_min_m.is_finite()
            || !range_max_m.is_finite()
            || range_min_m < 0.0
            || range_max_m < range_min_m
        {
            return Err(ComputeInputError::InvalidRangeBounds.into());
        }
        if !angle_min_rad.is_finite() || !angle_increment_rad.is_finite() {
            return Err(ComputeInputError::InvalidAngles.into());
        }
        if !sector_center_rad.is_finite()
            || !sector_half_width_rad.is_finite()
            || !(0.0..=core::f32::consts::PI).contains(&sector_half_width_rad)
        {
            return Err(ComputeInputError::InvalidSector.into());
        }
        if ranges_m.is_empty() {
            return Ok(CollisionSector::default());
        }
        let count = u32::try_from(ranges_m.len())
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        ensure_capacity(&self.stream, &mut self.lidar_ranges, ranges_m.len())?;
        if upload_ranges {
            self.stream
                .memcpy_htod(ranges_m, &mut self.lidar_ranges)
                .map_err(|error| backend_error("upload collision ranges", error))?;
        }
        self.stream
            .memcpy_htod(&[f32::INFINITY.to_bits()], &mut self.collision_minimum_bits)
            .map_err(|error| backend_error("reset collision minimum", error))?;
        self.stream
            .memcpy_htod(&[0_u32], &mut self.collision_sample_count)
            .map_err(|error| backend_error("reset collision sample count", error))?;
        {
            let ranges = self.lidar_ranges.slice(..ranges_m.len());
            unsafe {
                self.stream
                    .launch_builder(&self.collision_sector_reduce)
                    .arg(&ranges)
                    .arg(&mut self.collision_minimum_bits)
                    .arg(&mut self.collision_sample_count)
                    .arg(&count)
                    .arg(&angle_min_rad)
                    .arg(&angle_increment_rad)
                    .arg(&range_min_m)
                    .arg(&range_max_m)
                    .arg(&sector_center_rad)
                    .arg(&sector_half_width_rad)
                    .launch(LaunchConfig::for_num_elems(count))
            }
            .map_err(|error| backend_error("launch collision_sector_reduce", error))?;
        }
        let minimum_bits = self
            .stream
            .clone_dtoh(&self.collision_minimum_bits)
            .map_err(|error| backend_error("download collision minimum", error))?[0];
        let sample_count = self
            .stream
            .clone_dtoh(&self.collision_sample_count)
            .map_err(|error| backend_error("download collision sample count", error))?[0];
        Ok(CollisionSector {
            min_range_m: (sample_count != 0).then(|| f32::from_bits(minimum_bits)),
            sample_count,
        })
    }

    fn spatial_window(&mut self, scans: &[SpatialScan]) -> Result<Vec<LidarPoint>, WorkError> {
        let point_count = scans.iter().try_fold(0_usize, |total, scan| {
            total
                .checked_add(scan.ranges_m.len())
                .ok_or(ComputeInputError::LengthOverflow)
        })?;
        let count = u32::try_from(point_count)
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        if point_count == 0 {
            return Ok(Vec::new());
        }

        let mut ranges = Vec::with_capacity(point_count);
        let mut scan_indices = Vec::with_capacity(point_count);
        let mut local_indices = Vec::with_capacity(point_count);
        let mut angle_min = Vec::with_capacity(scans.len());
        let mut angle_increment = Vec::with_capacity(scans.len());
        let mut range_min = Vec::with_capacity(scans.len());
        let mut range_max = Vec::with_capacity(scans.len());
        let mut clockwise = Vec::with_capacity(scans.len());
        let mut pose_x = Vec::with_capacity(scans.len());
        let mut pose_y = Vec::with_capacity(scans.len());
        let mut pose_yaw = Vec::with_capacity(scans.len());
        for (scan_index, scan) in scans.iter().enumerate() {
            if !scan.range_min_m.is_finite()
                || !scan.range_max_m.is_finite()
                || scan.range_min_m < 0.0
                || scan.range_max_m < scan.range_min_m
            {
                return Err(ComputeInputError::InvalidRangeBounds.into());
            }
            if !scan.angle_min_rad.is_finite()
                || !scan.angle_increment_rad.is_finite()
                || !scan.pose_x_m.is_finite()
                || !scan.pose_y_m.is_finite()
                || !scan.pose_yaw_rad.is_finite()
            {
                return Err(ComputeInputError::InvalidAngles.into());
            }
            let scan_index = u32::try_from(scan_index)
                .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
            for (local_index, range) in scan.ranges_m.iter().copied().enumerate() {
                ranges.push(range);
                scan_indices.push(scan_index);
                local_indices.push(
                    u32::try_from(local_index)
                        .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?,
                );
            }
            angle_min.push(scan.angle_min_rad);
            angle_increment.push(scan.angle_increment_rad);
            range_min.push(scan.range_min_m);
            range_max.push(scan.range_max_m);
            clockwise.push(i32::from(scan.clockwise));
            pose_x.push(scan.pose_x_m);
            pose_y.push(scan.pose_y_m);
            pose_yaw.push(scan.pose_yaw_rad);
        }

        ensure_capacity(&self.stream, &mut self.lidar_ranges, point_count)?;
        ensure_capacity(&self.stream, &mut self.lidar_x, point_count)?;
        ensure_capacity(&self.stream, &mut self.lidar_y, point_count)?;
        ensure_capacity(&self.stream, &mut self.lidar_valid, point_count)?;
        ensure_capacity(&self.stream, &mut self.spatial_scan_indices, point_count)?;
        ensure_capacity(&self.stream, &mut self.spatial_local_indices, point_count)?;
        ensure_capacity(&self.stream, &mut self.spatial_angle_min, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_angle_increment, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_range_min, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_range_max, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_clockwise, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_pose_x, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_pose_y, scans.len())?;
        ensure_capacity(&self.stream, &mut self.spatial_pose_yaw, scans.len())?;

        macro_rules! upload {
            ($source:expr, $target:expr, $name:literal) => {
                self.stream
                    .memcpy_htod($source, $target)
                    .map_err(|error| backend_error($name, error))?
            };
        }
        upload!(&ranges, &mut self.lidar_ranges, "upload spatial ranges");
        upload!(
            &scan_indices,
            &mut self.spatial_scan_indices,
            "upload spatial scan indices"
        );
        upload!(
            &local_indices,
            &mut self.spatial_local_indices,
            "upload spatial local indices"
        );
        upload!(
            &angle_min,
            &mut self.spatial_angle_min,
            "upload spatial angle minimums"
        );
        upload!(
            &angle_increment,
            &mut self.spatial_angle_increment,
            "upload spatial angle increments"
        );
        upload!(
            &range_min,
            &mut self.spatial_range_min,
            "upload spatial range minimums"
        );
        upload!(
            &range_max,
            &mut self.spatial_range_max,
            "upload spatial range maximums"
        );
        upload!(
            &clockwise,
            &mut self.spatial_clockwise,
            "upload spatial scan directions"
        );
        upload!(&pose_x, &mut self.spatial_pose_x, "upload spatial pose x");
        upload!(&pose_y, &mut self.spatial_pose_y, "upload spatial pose y");
        upload!(
            &pose_yaw,
            &mut self.spatial_pose_yaw,
            "upload spatial pose yaw"
        );

        {
            let ranges = self.lidar_ranges.slice(..point_count);
            let scan_indices = self.spatial_scan_indices.slice(..point_count);
            let local_indices = self.spatial_local_indices.slice(..point_count);
            let angle_min = self.spatial_angle_min.slice(..scans.len());
            let angle_increment = self.spatial_angle_increment.slice(..scans.len());
            let range_min = self.spatial_range_min.slice(..scans.len());
            let range_max = self.spatial_range_max.slice(..scans.len());
            let clockwise = self.spatial_clockwise.slice(..scans.len());
            let pose_x = self.spatial_pose_x.slice(..scans.len());
            let pose_y = self.spatial_pose_y.slice(..scans.len());
            let pose_yaw = self.spatial_pose_yaw.slice(..scans.len());
            let mut x = self.lidar_x.slice_mut(..point_count);
            let mut y = self.lidar_y.slice_mut(..point_count);
            let mut valid = self.lidar_valid.slice_mut(..point_count);
            unsafe {
                self.stream
                    .launch_builder(&self.spatial_window_transform)
                    .arg(&ranges)
                    .arg(&scan_indices)
                    .arg(&local_indices)
                    .arg(&angle_min)
                    .arg(&angle_increment)
                    .arg(&range_min)
                    .arg(&range_max)
                    .arg(&clockwise)
                    .arg(&pose_x)
                    .arg(&pose_y)
                    .arg(&pose_yaw)
                    .arg(&mut x)
                    .arg(&mut y)
                    .arg(&mut valid)
                    .arg(&count)
                    .launch(LaunchConfig::for_num_elems(count))
            }
            .map_err(|error| backend_error("launch spatial_window_transform", error))?;
        }
        let x = self
            .stream
            .clone_dtoh(&self.lidar_x.slice(..point_count))
            .map_err(|error| backend_error("download spatial x", error))?;
        let y = self
            .stream
            .clone_dtoh(&self.lidar_y.slice(..point_count))
            .map_err(|error| backend_error("download spatial y", error))?;
        let valid = self
            .stream
            .clone_dtoh(&self.lidar_valid.slice(..point_count))
            .map_err(|error| backend_error("download spatial validity", error))?;
        Ok(x.into_iter()
            .zip(y)
            .zip(valid)
            .map(|((x_m, y_m), valid)| LidarPoint {
                x_m,
                y_m,
                valid: valid != 0,
            })
            .collect())
    }

    fn normalize_rgb(
        &mut self,
        input: &[u8],
        mean: [f32; 3],
        inverse_std: [f32; 3],
    ) -> Result<Vec<f32>, WorkError> {
        if !input.len().is_multiple_of(3)
            || mean.iter().any(|value| !value.is_finite())
            || inverse_std
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(ComputeInputError::InvalidNormalization.into());
        }
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let pixel_count = u32::try_from(input.len() / 3)
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        let value_count = u32::try_from(input.len())
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        ensure_capacity(&self.stream, &mut self.rgb_input, input.len())?;
        ensure_capacity(&self.stream, &mut self.rgb_output, input.len())?;
        self.stream
            .memcpy_htod(input, &mut self.rgb_input)
            .map_err(|error| backend_error("upload RGB input", error))?;
        {
            let input_view = self.rgb_input.slice(..input.len());
            let mut output_view = self.rgb_output.slice_mut(..input.len());
            unsafe {
                self.stream
                    .launch_builder(&self.normalize_rgb)
                    .arg(&input_view)
                    .arg(&mut output_view)
                    .arg(&pixel_count)
                    .arg(&mean[0])
                    .arg(&mean[1])
                    .arg(&mean[2])
                    .arg(&inverse_std[0])
                    .arg(&inverse_std[1])
                    .arg(&inverse_std[2])
                    .launch(LaunchConfig::for_num_elems(value_count))
            }
            .map_err(|error| backend_error("launch normalize_rgb_u8", error))?;
        }
        self.stream
            .clone_dtoh(&self.rgb_output.slice(..input.len()))
            .map_err(|error| backend_error("download normalized RGB", error))
    }

    #[allow(clippy::too_many_arguments)]
    fn predictive(
        &mut self,
        lower: &[f32],
        state: PredictiveState,
        top_down: &[f32],
        source_precision: f32,
        top_precision: f32,
    ) -> Result<PredictiveState, WorkError> {
        let count = state.state.len();
        if lower.len() != count
            || top_down.len() != count
            || state.weights.len() != count
            || state.bias.len() != count
        {
            return Err(ComputeInputError::LengthMismatch.into());
        }
        if count == 0 {
            return Ok(state);
        }
        let launch_count = u32::try_from(count)
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        ensure_capacity(&self.stream, &mut self.predictive_lower, count)?;
        ensure_capacity(&self.stream, &mut self.predictive_state, count)?;
        ensure_capacity(&self.stream, &mut self.predictive_top_down, count)?;
        ensure_capacity(&self.stream, &mut self.predictive_weights, count)?;
        ensure_capacity(&self.stream, &mut self.predictive_bias, count)?;
        self.stream
            .memcpy_htod(lower, &mut self.predictive_lower)
            .map_err(|error| backend_error("upload predictive lower", error))?;
        self.stream
            .memcpy_htod(&state.state, &mut self.predictive_state)
            .map_err(|error| backend_error("upload predictive state", error))?;
        self.stream
            .memcpy_htod(top_down, &mut self.predictive_top_down)
            .map_err(|error| backend_error("upload predictive top down", error))?;
        self.stream
            .memcpy_htod(&state.weights, &mut self.predictive_weights)
            .map_err(|error| backend_error("upload predictive weights", error))?;
        self.stream
            .memcpy_htod(&state.bias, &mut self.predictive_bias)
            .map_err(|error| backend_error("upload predictive bias", error))?;
        {
            let lower = self.predictive_lower.slice(..count);
            let mut state = self.predictive_state.slice_mut(..count);
            let top_down = self.predictive_top_down.slice(..count);
            let mut weights = self.predictive_weights.slice_mut(..count);
            let mut bias = self.predictive_bias.slice_mut(..count);
            unsafe {
                self.stream
                    .launch_builder(&self.predictive_step)
                    .arg(&lower)
                    .arg(&mut state)
                    .arg(&top_down)
                    .arg(&mut weights)
                    .arg(&mut bias)
                    .arg(&source_precision)
                    .arg(&top_precision)
                    .arg(&launch_count)
                    .launch(LaunchConfig::for_num_elems(launch_count))
            }
            .map_err(|error| backend_error("launch predictive_step", error))?;
        }
        Ok(PredictiveState {
            state: self
                .stream
                .clone_dtoh(&self.predictive_state.slice(..count))
                .map_err(|error| backend_error("download predictive state", error))?,
            weights: self
                .stream
                .clone_dtoh(&self.predictive_weights.slice(..count))
                .map_err(|error| backend_error("download predictive weights", error))?,
            bias: self
                .stream
                .clone_dtoh(&self.predictive_bias.slice(..count))
                .map_err(|error| backend_error("download predictive bias", error))?,
        })
    }

    fn cognition_load(&mut self, checkpoint: ResidentCognitionCheckpoint) -> Result<(), WorkError> {
        validate_cognition_checkpoint(&checkpoint)?;
        let dimension = checkpoint.sensor.len();
        let mut sensor = allocate_compute(&self.stream, dimension, "resident cognition sensor")?;
        let mut top_down =
            allocate_compute(&self.stream, dimension, "resident cognition top down")?;
        self.stream
            .memcpy_htod(&checkpoint.sensor, &mut sensor)
            .map_err(|error| backend_error("upload resident cognition sensor", error))?;
        self.stream
            .memcpy_htod(&checkpoint.top_down, &mut top_down)
            .map_err(|error| backend_error("upload resident cognition top down", error))?;
        let mut layers = Vec::with_capacity(checkpoint.layers.len());
        for source in checkpoint.layers {
            let mut activation =
                allocate_compute(&self.stream, dimension, "resident cognition activation")?;
            let mut weights =
                allocate_compute(&self.stream, dimension, "resident cognition weights")?;
            let mut bias = allocate_compute(&self.stream, dimension, "resident cognition bias")?;
            self.stream
                .memcpy_htod(&source.activation, &mut activation)
                .map_err(|error| backend_error("upload resident cognition activation", error))?;
            self.stream
                .memcpy_htod(&source.weights, &mut weights)
                .map_err(|error| backend_error("upload resident cognition weights", error))?;
            self.stream
                .memcpy_htod(&source.bias, &mut bias)
                .map_err(|error| backend_error("upload resident cognition bias", error))?;
            let (activation_mean, activation_rms) = host_activation_metrics(&source.activation);
            layers.push(CudaCognitionLayer {
                activation,
                weights,
                bias,
                sequence: source.sequence,
                precision: source.precision,
                prediction_error_l2: source.prediction_error_l2,
                activation_mean,
                activation_rms,
            });
        }
        self.cognition = Some(CudaCognitionState {
            dimension,
            sensor,
            top_down,
            layers,
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn cognition_advance(
        &mut self,
        sensor: &[f32],
        sensor_precision: f32,
        top_down: &[f32],
        top_precision: f32,
        due_layers: &[bool],
        snapshot_layer: Option<usize>,
    ) -> Result<CognitionStep, WorkError> {
        let cognition = self.cognition.as_mut().ok_or(WorkError::InvalidInput(
            ComputeInputError::InvalidCognitionState,
        ))?;
        if sensor.len() != cognition.dimension
            || top_down.len() != cognition.dimension
            || due_layers.len() != cognition.layers.len()
            || sensor
                .iter()
                .chain(top_down)
                .any(|value| !value.is_finite())
            || !valid_precision(sensor_precision)
            || !valid_precision(top_precision)
            || snapshot_layer.is_some_and(|layer| layer >= cognition.layers.len())
        {
            return Err(ComputeInputError::InvalidCognitionSchedule.into());
        }
        let count = u32::try_from(cognition.dimension)
            .map_err(|_| WorkError::InvalidInput(ComputeInputError::LengthOverflow))?;
        self.stream
            .memcpy_htod(sensor, &mut cognition.sensor)
            .map_err(|error| backend_error("upload resident cognition sensor", error))?;
        self.stream
            .memcpy_htod(top_down, &mut cognition.top_down)
            .map_err(|error| backend_error("upload resident cognition top down", error))?;

        for layer_index in 0..cognition.layers.len() {
            if !due_layers[layer_index] {
                continue;
            }
            let source_precision = if layer_index == 0 {
                sensor_precision
            } else {
                cognition.layers[layer_index - 1].precision
            };
            let upper_precision = if layer_index + 1 < cognition.layers.len() {
                cognition.layers[layer_index + 1].precision
            } else {
                top_precision
            };
            self.stream
                .memcpy_htod(&[0.0_f32; 3], &mut self.cognition_reductions)
                .map_err(|error| backend_error("reset cognition reductions", error))?;
            {
                let (before, current_and_after) = cognition.layers.split_at_mut(layer_index);
                let (current, after) = current_and_after
                    .split_first_mut()
                    .expect("validated resident layer index");
                let lower = if layer_index == 0 {
                    cognition.sensor.slice(..cognition.dimension)
                } else {
                    before[layer_index - 1]
                        .activation
                        .slice(..cognition.dimension)
                };
                let upper = if after.is_empty() {
                    cognition.top_down.slice(..cognition.dimension)
                } else {
                    after[0].activation.slice(..cognition.dimension)
                };
                let mut activation = current.activation.slice_mut(..cognition.dimension);
                let mut weights = current.weights.slice_mut(..cognition.dimension);
                let mut bias = current.bias.slice_mut(..cognition.dimension);
                unsafe {
                    self.stream
                        .launch_builder(&self.predictive_step_metrics)
                        .arg(&lower)
                        .arg(&mut activation)
                        .arg(&upper)
                        .arg(&mut weights)
                        .arg(&mut bias)
                        .arg(&source_precision)
                        .arg(&upper_precision)
                        .arg(&count)
                        .arg(&mut self.cognition_reductions)
                        .launch(LaunchConfig::for_num_elems(count))
                }
                .map_err(|error| backend_error("launch predictive_step_metrics", error))?;
            }
            let reductions = self
                .stream
                .clone_dtoh(&self.cognition_reductions)
                .map_err(|error| backend_error("download cognition layer metrics", error))?;
            let dimension = cognition.dimension as f32;
            let layer = &mut cognition.layers[layer_index];
            layer.prediction_error_l2 = (reductions[0] / dimension).sqrt();
            layer.precision =
                (source_precision / (1.0 + layer.prediction_error_l2)).clamp(0.0, 1.0);
            layer.activation_mean = reductions[1] / dimension;
            layer.activation_rms = (reductions[2] / dimension).sqrt();
            layer.sequence = layer.sequence.saturating_add(1);
        }

        let snapshot = snapshot_layer
            .map(|layer| {
                self.stream
                    .clone_dtoh(&cognition.layers[layer].activation)
                    .map(|activation| CognitionLayerSnapshot { layer, activation })
                    .map_err(|error| backend_error("download cognition snapshot", error))
            })
            .transpose()?;
        Ok(CognitionStep {
            layers: cognition
                .layers
                .iter()
                .map(|layer| CognitionLayerMetrics {
                    sequence: layer.sequence,
                    precision: layer.precision,
                    prediction_error_l2: layer.prediction_error_l2,
                    activation_mean: layer.activation_mean,
                    activation_rms: layer.activation_rms,
                })
                .collect(),
            snapshot,
        })
    }

    fn cognition_checkpoint(&self) -> Result<ResidentCognitionCheckpoint, WorkError> {
        let cognition = self.cognition.as_ref().ok_or(WorkError::InvalidInput(
            ComputeInputError::InvalidCognitionState,
        ))?;
        let sensor = self
            .stream
            .clone_dtoh(&cognition.sensor)
            .map_err(|error| backend_error("download checkpoint sensor", error))?;
        let top_down = self
            .stream
            .clone_dtoh(&cognition.top_down)
            .map_err(|error| backend_error("download checkpoint top down", error))?;
        let mut layers = Vec::with_capacity(cognition.layers.len());
        for source in &cognition.layers {
            layers.push(ResidentCognitionLayer {
                activation: self
                    .stream
                    .clone_dtoh(&source.activation)
                    .map_err(|error| backend_error("download checkpoint activation", error))?,
                weights: self
                    .stream
                    .clone_dtoh(&source.weights)
                    .map_err(|error| backend_error("download checkpoint weights", error))?,
                bias: self
                    .stream
                    .clone_dtoh(&source.bias)
                    .map_err(|error| backend_error("download checkpoint bias", error))?,
                sequence: source.sequence,
                precision: source.precision,
                prediction_error_l2: source.prediction_error_l2,
            });
        }
        Ok(ResidentCognitionCheckpoint {
            schema_version: crate::RESIDENT_COGNITION_SCHEMA_VERSION.to_string(),
            sensor,
            top_down,
            layers,
        })
    }
}

impl Backend for CudaBackend {
    fn execute(&mut self, job: ComputeJob) -> Result<ComputeResult, WorkError> {
        match job {
            ComputeJob::ProjectOccupancy { cells, depth } => {
                self.project(&cells, depth).map(ComputeResult::Occupancy)
            }
            ComputeJob::LidarTransform {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                yaw_offset_rad,
                clockwise,
            } => self
                .lidar(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    yaw_offset_rad,
                    clockwise,
                    true,
                )
                .map(ComputeResult::Lidar),
            ComputeJob::CollisionSectorReduce {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                sector_center_rad,
                sector_half_width_rad,
            } => self
                .collision_sector(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    sector_center_rad,
                    sector_half_width_rad,
                    true,
                )
                .map(ComputeResult::CollisionSector),
            ComputeJob::LidarTransformAndCollision {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                yaw_offset_rad,
                clockwise,
                sector_center_rad,
                sector_half_width_rad,
            } => {
                let lidar = self.lidar(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    yaw_offset_rad,
                    clockwise,
                    true,
                )?;
                let collision = self.collision_sector(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    sector_center_rad,
                    sector_half_width_rad,
                    false,
                )?;
                Ok(ComputeResult::Spatial { lidar, collision })
            }
            ComputeJob::SpatialWindowTransform { scans } => self
                .spatial_window(&scans)
                .map(ComputeResult::SpatialWindow),
            ComputeJob::NormalizeRgbU8 {
                input,
                mean,
                inverse_std,
            } => self
                .normalize_rgb(&input, mean, inverse_std)
                .map(ComputeResult::NormalizedRgb),
            ComputeJob::PredictiveStep {
                lower,
                state,
                top_down,
                source_precision,
                top_precision,
            } => self
                .predictive(&lower, state, &top_down, source_precision, top_precision)
                .map(ComputeResult::Predictive),
            ComputeJob::CognitionLoad { checkpoint } => {
                self.cognition_load(checkpoint)?;
                Ok(ComputeResult::CognitionLoaded)
            }
            ComputeJob::CognitionAdvance {
                sensor,
                sensor_precision,
                top_down,
                top_precision,
                due_layers,
                snapshot_layer,
            } => self
                .cognition_advance(
                    &sensor,
                    sensor_precision,
                    &top_down,
                    top_precision,
                    &due_layers,
                    snapshot_layer,
                )
                .map(ComputeResult::CognitionAdvanced),
            ComputeJob::CognitionCheckpoint => self
                .cognition_checkpoint()
                .map(ComputeResult::CognitionCheckpoint),
        }
    }
}

fn allocate_one<T>(
    stream: &Arc<CudaStream>,
    name: &'static str,
) -> Result<CudaSlice<T>, DeviceError>
where
    T: DeviceRepr + ValidAsZeroBits,
{
    stream
        .alloc_zeros(1)
        .map_err(|error| device_error(name, error))
}

fn ensure_capacity<T>(
    stream: &Arc<CudaStream>,
    buffer: &mut CudaSlice<T>,
    needed: usize,
) -> Result<(), WorkError>
where
    T: DeviceRepr + ValidAsZeroBits,
{
    if buffer.len() >= needed {
        return Ok(());
    }
    let capacity = needed.next_power_of_two();
    *buffer = stream
        .alloc_zeros(capacity)
        .map_err(|error| backend_error("grow persistent CUDA buffer", error))?;
    Ok(())
}

fn allocate_compute<T>(
    stream: &Arc<CudaStream>,
    count: usize,
    context: &'static str,
) -> Result<CudaSlice<T>, WorkError>
where
    T: DeviceRepr + ValidAsZeroBits,
{
    stream
        .alloc_zeros(count)
        .map_err(|error| backend_error(context, error))
}

fn host_activation_metrics(activation: &[f32]) -> (f32, f32) {
    let dimension = activation.len() as f32;
    let mean = activation.iter().sum::<f32>() / dimension;
    let rms = (activation.iter().map(|value| value * value).sum::<f32>() / dimension).sqrt();
    (mean, rms)
}

fn valid_precision(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn backend_error(context: &'static str, error: impl fmt::Display) -> WorkError {
    WorkError::Backend(format!("{context}: {error}"))
}
