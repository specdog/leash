use std::{fmt, sync::Arc};

use cudarc::{
    driver::{
        CudaContext, CudaFunction, CudaSlice, CudaStream, DeviceRepr, LaunchConfig, PushKernelArg,
        ValidAsZeroBits,
    },
    nvrtc::Ptx,
};

use crate::{
    executor::{Backend, ComputeJob, ComputeResult, PredictiveState, WorkError},
    CollisionSector, ComputeInputError, LidarPoint, PREBUILT_FATBIN,
};

#[derive(Debug)]
pub(crate) struct DeviceError(String);

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
    collision_sector_reduce: CudaFunction,
    normalize_rgb: CudaFunction,
    predictive_step: CudaFunction,
    occupancy_cells: CudaSlice<i8>,
    occupancy_output: CudaSlice<i32>,
    lidar_ranges: CudaSlice<f32>,
    lidar_x: CudaSlice<f32>,
    lidar_y: CudaSlice<f32>,
    lidar_valid: CudaSlice<u8>,
    collision_minimum_bits: CudaSlice<u32>,
    collision_sample_count: CudaSlice<u32>,
    rgb_input: CudaSlice<u8>,
    rgb_output: CudaSlice<f32>,
    predictive_lower: CudaSlice<f32>,
    predictive_state: CudaSlice<f32>,
    predictive_top_down: CudaSlice<f32>,
    predictive_weights: CudaSlice<f32>,
    predictive_bias: CudaSlice<f32>,
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
        let collision_sector_reduce = module
            .load_function("collision_sector_reduce")
            .map_err(|error| device_error("load collision_sector_reduce", error))?;
        let normalize_rgb = module
            .load_function("normalize_rgb_u8")
            .map_err(|error| device_error("load normalize_rgb_u8", error))?;
        let predictive_step = module
            .load_function("predictive_step")
            .map_err(|error| device_error("load predictive_step", error))?;

        Ok(Self {
            occupancy_cells: allocate_one(&stream, "occupancy cells")?,
            occupancy_output: allocate_one(&stream, "occupancy output")?,
            lidar_ranges: allocate_one(&stream, "lidar ranges")?,
            lidar_x: allocate_one(&stream, "lidar x")?,
            lidar_y: allocate_one(&stream, "lidar y")?,
            lidar_valid: allocate_one(&stream, "lidar validity")?,
            collision_minimum_bits: allocate_one(&stream, "collision minimum")?,
            collision_sample_count: allocate_one(&stream, "collision sample count")?,
            rgb_input: allocate_one(&stream, "RGB input")?,
            rgb_output: allocate_one(&stream, "RGB output")?,
            predictive_lower: allocate_one(&stream, "predictive lower")?,
            predictive_state: allocate_one(&stream, "predictive state")?,
            predictive_top_down: allocate_one(&stream, "predictive top down")?,
            predictive_weights: allocate_one(&stream, "predictive weights")?,
            predictive_bias: allocate_one(&stream, "predictive bias")?,
            stream,
            project_occupancy,
            lidar_transform,
            collision_sector_reduce,
            normalize_rgb,
            predictive_step,
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

fn backend_error(context: &'static str, error: impl fmt::Display) -> WorkError {
    WorkError::Backend(format!("{context}: {error}"))
}
