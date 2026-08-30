#include <cuda_runtime.h>
#include <math_constants.h>
#include <stdint.h>

extern "C" __global__ void project_occupancy(
    const int8_t* cells,
    int32_t* output,
    uint32_t cell_count,
    uint32_t depth
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t output_count = cell_count * depth;
    if (index >= output_count) {
        return;
    }
    const int8_t occupancy = cells[index / depth];
    output[index] = occupancy > 0 ? static_cast<int32_t>(occupancy) : 0;
}

extern "C" __global__ void lidar_transform(
    const float* ranges_m,
    float* x_m,
    float* y_m,
    uint8_t* valid,
    uint32_t count,
    float angle_min_rad,
    float angle_increment_rad,
    float range_min_m,
    float range_max_m,
    float yaw_offset_rad,
    int32_t clockwise
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) {
        return;
    }
    const float range = ranges_m[index];
    if (!isfinite(range) || range < range_min_m || range > range_max_m) {
        x_m[index] = 0.0f;
        y_m[index] = 0.0f;
        valid[index] = 0;
        return;
    }
    const float direction = clockwise != 0 ? -1.0f : 1.0f;
    const float angle = yaw_offset_rad
        + direction * (angle_min_rad + static_cast<float>(index) * angle_increment_rad);
    x_m[index] = range * cosf(angle);
    y_m[index] = range * sinf(angle);
    valid[index] = 1;
}

extern "C" __global__ void spatial_window_transform(
    const float* ranges_m,
    const uint32_t* scan_indices,
    const uint32_t* local_indices,
    const float* scan_params,
    float* x_m,
    float* y_m,
    uint8_t* valid,
    uint32_t count
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) {
        return;
    }
    const uint32_t scan = scan_indices[index];
    const float* params = &scan_params[scan * 8U];
    const float range = ranges_m[index];
    if (!isfinite(range)
        || range < params[2]
        || range > params[3]) {
        x_m[index] = 0.0f;
        y_m[index] = 0.0f;
        valid[index] = 0;
        return;
    }
    const float direction = params[4];
    const float angle = params[7]
        + direction * (
            params[0]
            + static_cast<float>(local_indices[index]) * params[1]
        );
    x_m[index] = params[5] + range * cosf(angle);
    y_m[index] = params[6] + range * sinf(angle);
    valid[index] = 1;
}

extern "C" __global__ void collision_sector_reduce(
    const float* ranges_m,
    uint32_t* minimum_range_bits,
    uint32_t* sample_count,
    uint32_t count,
    float angle_min_rad,
    float angle_increment_rad,
    float range_min_m,
    float range_max_m,
    float sector_center_rad,
    float sector_half_width_rad
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) {
        return;
    }
    const float range = ranges_m[index];
    if (!isfinite(range) || range < range_min_m || range > range_max_m) {
        return;
    }
    const float angle = angle_min_rad + static_cast<float>(index) * angle_increment_rad;
    const float unwrapped_delta = angle - sector_center_rad;
    const float delta = atan2f(sinf(unwrapped_delta), cosf(unwrapped_delta));
    if (fabsf(delta) > sector_half_width_rad) {
        return;
    }
    const float non_negative_range = range == 0.0f ? 0.0f : range;
    atomicMin(minimum_range_bits, __float_as_uint(non_negative_range));
    atomicAdd(sample_count, 1U);
}

extern "C" __global__ void normalize_rgb_u8(
    const uint8_t* input,
    float* output,
    uint32_t pixel_count,
    float mean_r,
    float mean_g,
    float mean_b,
    float inv_std_r,
    float inv_std_g,
    float inv_std_b
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t value_count = pixel_count * 3;
    if (index >= value_count) {
        return;
    }
    const uint32_t channel = index % 3;
    const float value = static_cast<float>(input[index]) / 255.0f;
    const float mean = channel == 0 ? mean_r : (channel == 1 ? mean_g : mean_b);
    const float inv_std = channel == 0
        ? inv_std_r
        : (channel == 1 ? inv_std_g : inv_std_b);
    output[index] = (value - mean) * inv_std;
}

extern "C" __global__ void predictive_step(
    const float* lower,
    float* state,
    const float* top_down,
    float* weights,
    float* bias,
    float source_precision,
    float top_precision,
    uint32_t count
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) {
        return;
    }
    const float previous = state[index];
    const float prediction = weights[index] * previous + bias[index];
    const float bottom_up_error = lower[index] - prediction;
    const float top_down_error = previous - top_down[index];
    const float next = previous
        + 0.12f * source_precision * weights[index] * bottom_up_error
        - 0.05f * top_precision * top_down_error;
    state[index] = fminf(4.0f, fmaxf(-4.0f, next));
    weights[index] = fminf(
        1.8f,
        fmaxf(0.2f, weights[index] + 0.0005f * bottom_up_error * previous)
    );
    bias[index] = fminf(
        1.0f,
        fmaxf(-1.0f, bias[index] + 0.0001f * bottom_up_error)
    );
}

extern "C" __global__ void predictive_step_metrics(
    const float* lower,
    float* state,
    const float* top_down,
    float* weights,
    float* bias,
    float source_precision,
    float top_precision,
    uint32_t count,
    float* reductions
) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= count) {
        return;
    }
    const float previous = state[index];
    const float prediction = weights[index] * previous + bias[index];
    const float bottom_up_error = lower[index] - prediction;
    const float top_down_error = previous - top_down[index];
    const float next = fminf(
        4.0f,
        fmaxf(
            -4.0f,
            previous
                + 0.12f * source_precision * weights[index] * bottom_up_error
                - 0.05f * top_precision * top_down_error
        )
    );
    state[index] = next;
    weights[index] = fminf(
        1.8f,
        fmaxf(0.2f, weights[index] + 0.0005f * bottom_up_error * previous)
    );
    bias[index] = fminf(
        1.0f,
        fmaxf(-1.0f, bias[index] + 0.0001f * bottom_up_error)
    );
    atomicAdd(&reductions[0], bottom_up_error * bottom_up_error);
    atomicAdd(&reductions[1], next);
    atomicAdd(&reductions[2], next * next);
}
