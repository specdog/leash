use std::sync::Arc;

use anyhow::{Context, Result};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::compile_ptx,
};

use crate::cognition::{COGNITION_STATE_DIM, LEASH_LAYER_COUNT};

const KERNEL: &str = r#"
extern "C" __global__ void predictive_step(
    const float* lower,
    float* state,
    const float* top_down,
    float* weights,
    float* bias,
    float source_precision,
    float top_precision,
    unsigned int count
) {
    unsigned int index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) return;
    float previous = state[index];
    float prediction = weights[index] * previous + bias[index];
    float bottom_up_error = lower[index] - prediction;
    float top_down_error = previous - top_down[index];
    float next = previous
        + 0.12f * source_precision * weights[index] * bottom_up_error
        - 0.05f * top_precision * top_down_error;
    state[index] = fminf(4.0f, fmaxf(-4.0f, next));
    weights[index] = fminf(
        1.8f,
        fmaxf(0.2f, weights[index] + 0.0005f * bottom_up_error * previous)
    );
    bias[index] = fminf(1.0f, fmaxf(-1.0f, bias[index] + 0.0001f * bottom_up_error));
}
"#;

pub struct CudaCognition {
    stream: Arc<CudaStream>,
    function: CudaFunction,
    sensor: CudaSlice<f32>,
    top_down: CudaSlice<f32>,
    layers: Vec<CudaSlice<f32>>,
    weights: Vec<CudaSlice<f32>>,
    biases: Vec<CudaSlice<f32>>,
}

impl CudaCognition {
    pub fn new() -> Result<Self> {
        let context = CudaContext::new(0).context("create CUDA cognition context")?;
        let stream = context.default_stream();
        let ptx = compile_ptx(KERNEL).context("compile cognition CUDA kernel")?;
        let module = context
            .load_module(ptx)
            .context("load cognition CUDA module")?;
        let function = module
            .load_function("predictive_step")
            .context("load predictive_step CUDA kernel")?;
        let sensor = stream
            .alloc_zeros(COGNITION_STATE_DIM)
            .context("allocate CUDA sensor state")?;
        let top_down = stream
            .alloc_zeros(COGNITION_STATE_DIM)
            .context("allocate CUDA top-down state")?;
        let mut layers = Vec::with_capacity(LEASH_LAYER_COUNT);
        let mut weights = Vec::with_capacity(LEASH_LAYER_COUNT);
        let mut biases = Vec::with_capacity(LEASH_LAYER_COUNT);
        for layer in 0..LEASH_LAYER_COUNT {
            layers.push(
                stream
                    .alloc_zeros(COGNITION_STATE_DIM)
                    .context("allocate CUDA layer state")?,
            );
            weights.push(
                stream
                    .clone_htod(&vec![0.75 + layer as f32 * 0.05; COGNITION_STATE_DIM])
                    .context("allocate CUDA layer weights")?,
            );
            biases.push(
                stream
                    .alloc_zeros(COGNITION_STATE_DIM)
                    .context("allocate CUDA layer bias")?,
            );
        }
        Ok(Self {
            stream,
            function,
            sensor,
            top_down,
            layers,
            weights,
            biases,
        })
    }

    pub fn update_sensor(&mut self, sensor: &[f32]) -> Result<()> {
        self.stream
            .memcpy_htod(sensor, &mut self.sensor)
            .context("copy cognition sensor state to CUDA")
    }

    pub fn update_top_down(&mut self, top_down: &[f32]) -> Result<()> {
        self.stream
            .memcpy_htod(top_down, &mut self.top_down)
            .context("copy cognition top-down state to CUDA")
    }

    pub fn step(&mut self, layer: usize, source_precision: f32, top_precision: f32) -> Result<()> {
        let (lower_layers, current_and_upper) = self.layers.split_at_mut(layer);
        let (current, upper_layers) = current_and_upper
            .split_first_mut()
            .context("invalid CUDA cognition layer")?;
        let lower = if layer == 0 {
            &self.sensor
        } else {
            &lower_layers[layer - 1]
        };
        let top_down = upper_layers.first().unwrap_or(&self.top_down);
        let weight = &mut self.weights[layer];
        let bias = &mut self.biases[layer];
        let count = COGNITION_STATE_DIM as u32;
        let launch = LaunchConfig::for_num_elems(count);
        unsafe {
            self.stream
                .launch_builder(&self.function)
                .arg(lower)
                .arg(current)
                .arg(top_down)
                .arg(weight)
                .arg(bias)
                .arg(&source_precision)
                .arg(&top_precision)
                .arg(&count)
                .launch(launch)
        }
        .context("launch CUDA predictive cognition step")
        .map(|_| ())
    }
}
