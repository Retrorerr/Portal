/* Native fixture for a Flatpak Platform; needs only SDL's runtime library. */
#include <stdint.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <vulkan/vulkan.h>

extern int SDL_Init(unsigned);
extern const char *SDL_GetError(void);
extern void *SDL_CreateWindow(const char *, int, int, int, int, unsigned);
extern void *SDL_GL_CreateContext(void *);
extern int SDL_GL_SetSwapInterval(int);
extern void *SDL_GL_GetProcAddress(const char *);
extern void SDL_GL_SwapWindow(void *);
extern int SDL_PollEvent(void *);
extern const char *SDL_GetCurrentVideoDriver(void);
extern void SDL_GL_DeleteContext(void *);
extern void SDL_DestroyWindow(void *);
extern void SDL_Quit(void);

static int64_t now_ns(void) {
    struct timespec time;
    clock_gettime(CLOCK_MONOTONIC, &time);
    return (int64_t)time.tv_sec * 1000000000 + time.tv_nsec;
}

static void json_string(const char *text) {
    putchar('"');
    for (; *text; text++) {
        unsigned char c = *text;
        if (c == '"' || c == '\\') putchar('\\');
        if (c < 32) printf("\\u%04x", c); else putchar(c);
    }
    putchar('"');
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    void *window = NULL, *context = NULL;
    int status = 1;
    fprintf(stderr, "PORTAL_FLATPAK_ENV FD_FORCE_KGSL=%s MESA_LOADER_DRIVER_OVERRIDE=%s GALLIUM_DRIVER=%s\n",
            getenv("FD_FORCE_KGSL"), getenv("MESA_LOADER_DRIVER_OVERRIDE"), getenv("GALLIUM_DRIVER"));
    int gpu_access = access("/dev/kgsl-3d0", R_OK | W_OK);
    fprintf(stderr, "PORTAL_FLATPAK_GPU kgsl_access=%d errno=%d\n", gpu_access, gpu_access ? errno : 0);
    fprintf(stderr, "PORTAL_FLATPAK_PHASE SDL_Init\n");
    if (SDL_Init(0x20)) goto done;
    fprintf(stderr, "PORTAL_FLATPAK_PHASE SDL_CreateWindow\n");
    window = SDL_CreateWindow("Portal compatibility: Flatpak SDL/OpenGL", 0x1FFF0000, 0x1FFF0000, 720, 480, 0x26);
    if (!window) goto done;
    fprintf(stderr, "PORTAL_FLATPAK_PHASE SDL_GL_CreateContext\n");
    if (!(context = SDL_GL_CreateContext(window))) goto done;
    fprintf(stderr, "PORTAL_FLATPAK_PHASE animate\n");
    SDL_GL_SetSwapInterval(1);
    const char *(*get_string)(unsigned) = SDL_GL_GetProcAddress("glGetString");
    void (*clear_color)(float, float, float, float) = SDL_GL_GetProcAddress("glClearColor");
    void (*clear)(unsigned) = SDL_GL_GetProcAddress("glClear");
    void (*read_pixels)(int, int, int, int, unsigned, unsigned, void *) = SDL_GL_GetProcAddress("glReadPixels");
    if (!get_string || !clear_color || !clear || !read_pixels) goto done;
    const char *renderer = get_string(0x1F01);
    if (!renderer) goto done;
    int accelerated = strstr(renderer, "Adreno") || strstr(renderer, "FD830");
    int denied = access(argv[2], F_OK) != 0;
    unsigned char pixel[4];
    clear_color(0.125f, 0.25f, 0.5f, 1);
    clear(0x4000);
    read_pixels(10, 10, 1, 1, 0x1908, 0x1401, pixel);
    int correct_pixels = abs(pixel[0] - 32) <= 1 && abs(pixel[1] - 64) <= 1 &&
                         abs(pixel[2] - 128) <= 1 && pixel[3] == 255;
    VkApplicationInfo application = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                                     .pApplicationName = "Portal compatibility", .apiVersion = VK_API_VERSION_1_0};
    VkInstanceCreateInfo create = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &application};
    VkInstance instance;
    int vulkan_hardware = 0;
    char vulkan_device[VK_MAX_PHYSICAL_DEVICE_NAME_SIZE] = "unavailable";
    if (vkCreateInstance(&create, NULL, &instance) == VK_SUCCESS) {
        uint32_t count = 16;
        VkPhysicalDevice devices[16];
        VkResult enumerated = vkEnumeratePhysicalDevices(instance, &count, devices);
        if (enumerated == VK_SUCCESS || enumerated == VK_INCOMPLETE) {
            for (uint32_t index = 0; index < count; index++) {
                VkPhysicalDeviceProperties properties;
                vkGetPhysicalDeviceProperties(devices[index], &properties);
                if (strstr(properties.deviceName, "Adreno")) {
                    vulkan_hardware = 1;
                    memcpy(vulkan_device, properties.deviceName, sizeof(vulkan_device));
                    vulkan_device[sizeof(vulkan_device) - 1] = 0;
                    break;
                }
            }
        }
        vkDestroyInstance(instance, NULL);
    }
    int64_t start = now_ns(), duration = (int64_t)(atof(argv[1]) * 1000000000);
    unsigned frames = 0;
    uint64_t event[8];
    while (now_ns() - start < duration) {
        while (SDL_PollEvent(event)) {}
        clear_color((frames % 120) / 120.0f, 0.2f, 0.4f, 1);
        clear(0x4000);
        SDL_GL_SwapWindow(window);
        frames++;
    }
    printf("{\"toolkit\":\"Flatpak SDL2\",\"backend\":");
    json_string(SDL_GetCurrentVideoDriver());
    printf(",\"renderer\":"); json_string(renderer);
    printf(",\"vulkan_device\":"); json_string(vulkan_device);
    printf(",\"hardware_gl\":%s,\"vulkan_hardware\":%s,\"correct_pixels\":%s,\"external_canary_denied\":%s,\"frames\":%u,\"flow_start_ns\":%lld,\"flow_end_ns\":%lld}\n",
           accelerated ? "true" : "false", vulkan_hardware ? "true" : "false", correct_pixels ? "true" : "false",
           denied ? "true" : "false", frames,
           (long long)start, (long long)now_ns());
    status = accelerated && vulkan_hardware && correct_pixels && denied && frames > 0 ? 0 : 1;
done:
    if (status) fprintf(stderr, "Flatpak SDL probe failed: %s\n", SDL_GetError());
    if (context) SDL_GL_DeleteContext(context);
    if (window) SDL_DestroyWindow(window);
    SDL_Quit();
    return status;
}
