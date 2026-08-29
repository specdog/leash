#include <cuda.h>

#include <cmath>
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <cstdlib>
#include <vector>

#define CUDA_CHECK(call)                                                        \
    do {                                                                        \
        CUresult result = (call);                                                \
        if (result != CUDA_SUCCESS) {                                            \
            const char* message = nullptr;                                       \
            cuGetErrorString(result, &message);                                  \
            std::fprintf(stderr, "%s failed: %s\n", #call,                     \
                         message == nullptr ? "unknown CUDA error" : message);   \
            return 1;                                                           \
        }                                                                       \
    } while (false)

static bool close_enough(float left, float right) {
    return std::fabs(left - right) <= 1e-5f;
}

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: jetson_driver_probe <leash_kernels.fatbin>\n");
        return 2;
    }

    CUDA_CHECK(cuInit(0));
    CUdevice device;
    CUDA_CHECK(cuDeviceGet(&device, 0));
    int major = 0;
    int minor = 0;
    CUDA_CHECK(cuDeviceGetAttribute(
        &major, CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR, device));
    CUDA_CHECK(cuDeviceGetAttribute(
        &minor, CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR, device));
    if (major != 8 || minor != 7) {
        std::fprintf(stderr, "expected compute capability 8.7, got %d.%d\n", major, minor);
        return 1;
    }

    CUcontext context;
    CUDA_CHECK(cuCtxCreate(&context, 0, device));
    CUmodule module;
    CUDA_CHECK(cuModuleLoad(&module, argv[1]));

    CUfunction occupancy;
    CUDA_CHECK(cuModuleGetFunction(&occupancy, module, "project_occupancy"));
    const std::int8_t cells[] = {-1, 0, 100};
    std::int32_t occupancy_output[6] = {};
    CUdeviceptr device_cells;
    CUdeviceptr device_occupancy;
    CUDA_CHECK(cuMemAlloc(&device_cells, sizeof(cells)));
    CUDA_CHECK(cuMemAlloc(&device_occupancy, sizeof(occupancy_output)));
    CUDA_CHECK(cuMemcpyHtoD(device_cells, cells, sizeof(cells)));
    std::uint32_t cell_count = 3;
    std::uint32_t depth = 2;
    void* occupancy_args[] = {&device_cells, &device_occupancy, &cell_count, &depth};
    CUDA_CHECK(cuLaunchKernel(occupancy, 1, 1, 1, 128, 1, 1, 0, nullptr,
                              occupancy_args, nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(occupancy_output, device_occupancy,
                            sizeof(occupancy_output)));
    const std::int32_t expected_occupancy[] = {0, 0, 0, 0, 100, 100};
    for (std::size_t index = 0; index < 6; ++index) {
        if (occupancy_output[index] != expected_occupancy[index]) {
            std::fprintf(stderr, "occupancy mismatch at %zu\n", index);
            return 1;
        }
    }

    CUfunction lidar;
    CUDA_CHECK(cuModuleGetFunction(&lidar, module, "lidar_transform"));
    const float ranges[] = {1.0f, NAN, 20.0f};
    float lidar_x[3] = {};
    float lidar_y[3] = {};
    std::uint8_t lidar_valid[3] = {};
    CUdeviceptr device_ranges;
    CUdeviceptr device_x;
    CUdeviceptr device_y;
    CUdeviceptr device_valid;
    CUDA_CHECK(cuMemAlloc(&device_ranges, sizeof(ranges)));
    CUDA_CHECK(cuMemAlloc(&device_x, sizeof(lidar_x)));
    CUDA_CHECK(cuMemAlloc(&device_y, sizeof(lidar_y)));
    CUDA_CHECK(cuMemAlloc(&device_valid, sizeof(lidar_valid)));
    CUDA_CHECK(cuMemcpyHtoD(device_ranges, ranges, sizeof(ranges)));
    std::uint32_t lidar_count = 3;
    float angle_min = 0.0f;
    float angle_increment = 1.57079632679f;
    float range_min = 0.05f;
    float range_max = 12.0f;
    float yaw_offset = 0.0f;
    std::int32_t clockwise = 0;
    void* lidar_args[] = {&device_ranges, &device_x, &device_y, &device_valid,
                          &lidar_count, &angle_min, &angle_increment, &range_min,
                          &range_max, &yaw_offset, &clockwise};
    CUDA_CHECK(cuLaunchKernel(lidar, 1, 1, 1, 128, 1, 1, 0, nullptr, lidar_args,
                              nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(lidar_x, device_x, sizeof(lidar_x)));
    CUDA_CHECK(cuMemcpyDtoH(lidar_y, device_y, sizeof(lidar_y)));
    CUDA_CHECK(cuMemcpyDtoH(lidar_valid, device_valid, sizeof(lidar_valid)));
    if (lidar_valid[0] != 1 || !close_enough(lidar_x[0], 1.0f) ||
        !close_enough(lidar_y[0], 0.0f) || lidar_valid[1] != 0 ||
        lidar_valid[2] != 0) {
        std::fprintf(stderr, "lidar parity failed\n");
        return 1;
    }

    CUfunction collision;
    CUDA_CHECK(cuModuleGetFunction(&collision, module, "collision_sector_reduce"));
    CUdeviceptr device_minimum_bits;
    CUdeviceptr device_sample_count;
    CUDA_CHECK(cuMemAlloc(&device_minimum_bits, sizeof(std::uint32_t)));
    CUDA_CHECK(cuMemAlloc(&device_sample_count, sizeof(std::uint32_t)));
    float infinity = INFINITY;
    std::uint32_t minimum_bits = 0;
    static_assert(sizeof(infinity) == sizeof(minimum_bits));
    std::memcpy(&minimum_bits, &infinity, sizeof(minimum_bits));
    std::uint32_t collision_count = 0;
    CUDA_CHECK(cuMemcpyHtoD(device_minimum_bits, &minimum_bits, sizeof(minimum_bits)));
    CUDA_CHECK(cuMemcpyHtoD(device_sample_count, &collision_count, sizeof(collision_count)));
    float sector_center = 0.0f;
    float sector_half_width = 0.25f;
    void* collision_args[] = {
        &device_ranges, &device_minimum_bits, &device_sample_count, &lidar_count,
        &angle_min, &angle_increment, &range_min, &range_max, &sector_center,
        &sector_half_width};
    CUDA_CHECK(cuLaunchKernel(collision, 1, 1, 1, 128, 1, 1, 0, nullptr,
                              collision_args, nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(&minimum_bits, device_minimum_bits, sizeof(minimum_bits)));
    CUDA_CHECK(cuMemcpyDtoH(&collision_count, device_sample_count,
                            sizeof(collision_count)));
    float minimum = 0.0f;
    std::memcpy(&minimum, &minimum_bits, sizeof(minimum));
    if (collision_count != 1 || !close_enough(minimum, 1.0f)) {
        std::fprintf(stderr, "collision reduction parity failed\n");
        return 1;
    }

    CUfunction normalize;
    CUDA_CHECK(cuModuleGetFunction(&normalize, module, "normalize_rgb_u8"));
    const std::uint8_t rgb[] = {0, 127, 255};
    float normalized[3] = {};
    CUdeviceptr device_rgb;
    CUdeviceptr device_normalized;
    CUDA_CHECK(cuMemAlloc(&device_rgb, sizeof(rgb)));
    CUDA_CHECK(cuMemAlloc(&device_normalized, sizeof(normalized)));
    CUDA_CHECK(cuMemcpyHtoD(device_rgb, rgb, sizeof(rgb)));
    std::uint32_t pixel_count = 1;
    float mean_r = 0.5f;
    float mean_g = 0.5f;
    float mean_b = 0.5f;
    float inv_std_r = 2.0f;
    float inv_std_g = 2.0f;
    float inv_std_b = 2.0f;
    void* normalize_args[] = {&device_rgb, &device_normalized, &pixel_count,
                              &mean_r, &mean_g, &mean_b, &inv_std_r, &inv_std_g,
                              &inv_std_b};
    CUDA_CHECK(cuLaunchKernel(normalize, 1, 1, 1, 128, 1, 1, 0, nullptr,
                              normalize_args, nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(normalized, device_normalized, sizeof(normalized)));
    if (!close_enough(normalized[0], -1.0f) ||
        std::fabs(normalized[1]) > 0.01f ||
        !close_enough(normalized[2], 1.0f)) {
        std::fprintf(stderr, "RGB normalization parity failed\n");
        return 1;
    }

    CUfunction predictive;
    CUDA_CHECK(cuModuleGetFunction(&predictive, module, "predictive_step"));
    const float lower[] = {1.0f, -1.0f};
    float state[] = {0.5f, -0.5f};
    const float top_down[] = {0.25f, -0.25f};
    float weights[] = {0.75f, 0.75f};
    float bias[] = {0.0f, 0.0f};
    CUdeviceptr device_lower;
    CUdeviceptr device_state;
    CUdeviceptr device_top_down;
    CUdeviceptr device_weights;
    CUdeviceptr device_bias;
    CUDA_CHECK(cuMemAlloc(&device_lower, sizeof(lower)));
    CUDA_CHECK(cuMemAlloc(&device_state, sizeof(state)));
    CUDA_CHECK(cuMemAlloc(&device_top_down, sizeof(top_down)));
    CUDA_CHECK(cuMemAlloc(&device_weights, sizeof(weights)));
    CUDA_CHECK(cuMemAlloc(&device_bias, sizeof(bias)));
    CUDA_CHECK(cuMemcpyHtoD(device_lower, lower, sizeof(lower)));
    CUDA_CHECK(cuMemcpyHtoD(device_state, state, sizeof(state)));
    CUDA_CHECK(cuMemcpyHtoD(device_top_down, top_down, sizeof(top_down)));
    CUDA_CHECK(cuMemcpyHtoD(device_weights, weights, sizeof(weights)));
    CUDA_CHECK(cuMemcpyHtoD(device_bias, bias, sizeof(bias)));
    float source_precision = 1.0f;
    float top_precision = 0.5f;
    std::uint32_t predictive_count = 2;
    void* predictive_args[] = {&device_lower, &device_state, &device_top_down,
                               &device_weights, &device_bias, &source_precision,
                               &top_precision, &predictive_count};
    CUDA_CHECK(cuLaunchKernel(predictive, 1, 1, 1, 128, 1, 1, 0, nullptr,
                              predictive_args, nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(state, device_state, sizeof(state)));
    CUDA_CHECK(cuMemcpyDtoH(weights, device_weights, sizeof(weights)));
    CUDA_CHECK(cuMemcpyDtoH(bias, device_bias, sizeof(bias)));
    if (!(state[0] > 0.5f) || !(state[1] < -0.5f) ||
        close_enough(weights[0], 0.75f) || close_enough(bias[0], 0.0f)) {
        std::fprintf(stderr, "predictive step parity failed\n");
        return 1;
    }

    CUfunction predictive_metrics;
    CUDA_CHECK(cuModuleGetFunction(&predictive_metrics, module,
                                   "predictive_step_metrics"));
    const float initial_state[] = {0.5f, -0.5f};
    const float initial_weights[] = {0.75f, 0.75f};
    const float initial_bias[] = {0.0f, 0.0f};
    float reductions[] = {0.0f, 0.0f, 0.0f};
    CUdeviceptr device_reductions;
    CUDA_CHECK(cuMemAlloc(&device_reductions, sizeof(reductions)));
    CUDA_CHECK(cuMemcpyHtoD(device_state, initial_state, sizeof(initial_state)));
    CUDA_CHECK(cuMemcpyHtoD(device_weights, initial_weights, sizeof(initial_weights)));
    CUDA_CHECK(cuMemcpyHtoD(device_bias, initial_bias, sizeof(initial_bias)));
    CUDA_CHECK(cuMemcpyHtoD(device_reductions, reductions, sizeof(reductions)));
    void* predictive_metrics_args[] = {
        &device_lower, &device_state, &device_top_down, &device_weights,
        &device_bias, &source_precision, &top_precision, &predictive_count,
        &device_reductions};
    CUDA_CHECK(cuLaunchKernel(predictive_metrics, 1, 1, 1, 128, 1, 1, 0,
                              nullptr, predictive_metrics_args, nullptr));
    CUDA_CHECK(cuCtxSynchronize());
    CUDA_CHECK(cuMemcpyDtoH(state, device_state, sizeof(state)));
    CUDA_CHECK(cuMemcpyDtoH(reductions, device_reductions, sizeof(reductions)));
    if (!(state[0] > 0.5f) || !(state[1] < -0.5f) || !(reductions[0] > 0.0f) ||
        !std::isfinite(reductions[1]) || !(reductions[2] > 0.0f)) {
        std::fprintf(stderr, "predictive metrics parity failed\n");
        return 1;
    }

    CUDA_CHECK(cuModuleUnload(module));
    CUDA_CHECK(cuCtxDestroy(context));
    std::printf("leash CUDA probe passed: sm_%d%d, 6 kernels\n", major, minor);
    return 0;
}
