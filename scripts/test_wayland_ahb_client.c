#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <time.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <math.h>
#include <android/hardware_buffer.h>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES2/gl2ext.h>

#define EGL_NATIVE_BUFFER_ANDROID 0x3140

typedef struct native_handle {
    int version;
    int numFds;
    int numInts;
    int data[0];
} native_handle_t;

extern const native_handle_t* AHardwareBuffer_getNativeHandle(const AHardwareBuffer* buffer);

static PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC peglGetNativeClientBufferANDROID = NULL;
static PFNEGLCREATEIMAGEKHRPROC peglCreateImageKHR = NULL;
static PFNEGLDESTROYIMAGEKHRPROC peglDestroyImageKHR = NULL;
static PFNGLEGLIMAGETARGETTEXTURE2DOESPROC pglEGLImageTargetTexture2DOES = NULL;

struct wl_msg_hdr {
    uint32_t id;
    uint16_t opcode;
    uint16_t size;
};

static void wl_send_raw(int sock, uint32_t id, uint16_t opcode, const void* payload, uint16_t payload_len) {
    uint16_t total_len = sizeof(struct wl_msg_hdr) + payload_len;
    struct wl_msg_hdr hdr = { id, opcode, total_len };
    struct iovec iov[2];
    iov[0].iov_base = &hdr;
    iov[0].iov_len = sizeof(hdr);
    iov[1].iov_base = (void*)payload;
    iov[1].iov_len = payload_len;

    struct msghdr msg = {0};
    msg.msg_iov = iov;
    msg.msg_iovlen = payload_len > 0 ? 2 : 1;

    ssize_t sent = sendmsg(sock, &msg, MSG_NOSIGNAL);
    if (sent < 0) {
        fprintf(stderr, "sendmsg failed: %s\n", strerror(errno));
    }
}

static void wl_send_fd(int sock, uint32_t id, uint16_t opcode, int fd_to_send) {
    uint16_t total_len = sizeof(struct wl_msg_hdr);
    struct wl_msg_hdr hdr = { id, opcode, total_len };
    struct iovec iov = { &hdr, sizeof(hdr) };

    char cmsg_buf[CMSG_SPACE(sizeof(int))] = {0};
    struct msghdr msg = {0};
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf;
    msg.msg_controllen = sizeof(cmsg_buf);

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    *(int*)CMSG_DATA(cmsg) = fd_to_send;

    ssize_t sent = sendmsg(sock, &msg, MSG_NOSIGNAL);
    if (sent < 0) {
        fprintf(stderr, "sendmsg with fd failed: %s\n", strerror(errno));
    }
}

// Global registry tracking
static uint32_t compositor_id = 0;
static uint32_t xdg_wm_base_id = 0;
static uint32_t android_wlegl_id = 0;

static uint32_t next_id = 3;


static void parse_registry_global(int sock, const uint8_t* payload, uint16_t len) {
    if (len < 8) return;
    uint32_t name = *(uint32_t*)(payload);
    uint32_t str_len = *(uint32_t*)(payload + 4);
    if (8 + str_len > len) return;
    const char* iface = (const char*)(payload + 8);
    uint32_t padded_str_len = (str_len + 3) & ~3;
    uint32_t version = *(uint32_t*)(payload + 8 + padded_str_len);

    printf("Registry global: name=%u iface=%s version=%u\n", name, iface, version);

    if (strcmp(iface, "wl_compositor") == 0 && compositor_id == 0) {
        compositor_id = next_id++;
        // bind(name, iface, version, id)
        uint32_t bind_buf[256];
        int idx = 0;
        bind_buf[idx++] = name;
        uint32_t slen = strlen(iface) + 1;
        bind_buf[idx++] = slen;
        memcpy(&bind_buf[idx], iface, slen);
        idx += (slen + 3) / 4;
        bind_buf[idx++] = (version > 4) ? 4 : version;
        bind_buf[idx++] = compositor_id;
        wl_send_raw(sock, 2, 0, bind_buf, idx * 4);
        printf("Bound wl_compositor -> id %u\n", compositor_id);
    } else if (strcmp(iface, "xdg_wm_base") == 0 && xdg_wm_base_id == 0) {
        xdg_wm_base_id = next_id++;
        uint32_t bind_buf[256];
        int idx = 0;
        bind_buf[idx++] = name;
        uint32_t slen = strlen(iface) + 1;
        bind_buf[idx++] = slen;
        memcpy(&bind_buf[idx], iface, slen);
        idx += (slen + 3) / 4;
        bind_buf[idx++] = 1; // version 1
        bind_buf[idx++] = xdg_wm_base_id;
        wl_send_raw(sock, 2, 0, bind_buf, idx * 4);
        printf("Bound xdg_wm_base -> id %u\n", xdg_wm_base_id);
    } else if (strcmp(iface, "android_wlegl") == 0 && android_wlegl_id == 0) {
        android_wlegl_id = next_id++;
        uint32_t bind_buf[256];
        int idx = 0;
        bind_buf[idx++] = name;
        uint32_t slen = strlen(iface) + 1;
        bind_buf[idx++] = slen;
        memcpy(&bind_buf[idx], iface, slen);
        idx += (slen + 3) / 4;
        bind_buf[idx++] = 1; // version 1
        bind_buf[idx++] = android_wlegl_id;
        wl_send_raw(sock, 2, 0, bind_buf, idx * 4);
        printf("Bound android_wlegl -> id %u\n", android_wlegl_id);
    }
}

// Shader sources
static const char* VERTEX_SHADER =
    "#version 300 es\n"
    "layout(location = 0) in vec2 aPos;\n"
    "layout(location = 1) in vec3 aColor;\n"
    "uniform float uAngle;\n"
    "out vec3 vColor;\n"
    "void main() {\n"
    "    float c = cos(uAngle);\n"
    "    float s = sin(uAngle);\n"
    "    mat2 rot = mat2(c, -s, s, c);\n"
    "    gl_Position = vec4(rot * aPos, 0.0, 1.0);\n"
    "    vColor = aColor;\n"
    "}\n";

static const char* FRAGMENT_SHADER =
    "#version 300 es\n"
    "precision mediump float;\n"
    "in vec3 vColor;\n"
    "out vec4 FragColor;\n"
    "void main() {\n"
    "    FragColor = vec4(vColor, 1.0);\n"
    "}\n";

static GLuint compile_shader(GLenum type, const char* src) {
    GLuint s = glCreateShader(type);
    glShaderSource(s, 1, &src, NULL);
    glCompileShader(s);
    GLint ok;
    glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        char log[512];
        glGetShaderInfoLog(s, sizeof(log), NULL, log);
        fprintf(stderr, "Shader compile error: %s\n", log);
        return 0;
    }
    return s;
}

#define NUM_BUFFERS 3
#define DEFAULT_WIN_W 800
#define DEFAULT_WIN_H 600
#define DEFAULT_FRAME_LIMIT 300
#define DEFAULT_HOLD_SECONDS 5

struct BufferSlot {
    AHardwareBuffer* ahb;
    uint32_t wl_buffer_id;
    GLuint fbo;
    GLuint tex;
    int busy;
};

int main(int argc, char** argv) {
    printf("=== Starting Phase 1 Wayland GLES Hardware Acceleration Client ===\n");

    const char* socket_path = "/data/data/app.polarbear/files/runtime-B/tmp/wayland-0";
    if (argc > 1) {
        socket_path = argv[1];
    }
    int frame_limit = argc > 2 ? atoi(argv[2]) : DEFAULT_FRAME_LIMIT;
    int win_w = argc > 3 ? atoi(argv[3]) : DEFAULT_WIN_W;
    int win_h = argc > 4 ? atoi(argv[4]) : DEFAULT_WIN_H;
    int hold_seconds = argc > 5 ? atoi(argv[5]) : DEFAULT_HOLD_SECONDS;
    if (frame_limit <= 0 || win_w <= 0 || win_h <= 0 || hold_seconds < 0) {
        fprintf(stderr, "Invalid arguments: frames=%d size=%dx%d hold=%d\n",
                frame_limit, win_w, win_h, hold_seconds);
        return 2;
    }

    printf("Connecting to Wayland socket: %s\n", socket_path);
    int sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (sock < 0) {
        perror("socket");
        return 1;
    }

    struct sockaddr_un addr = {0};
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, socket_path, sizeof(addr.sun_path) - 1);

    if (connect(sock, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        perror("connect");
        return 1;
    }
    printf("Connected to Smithay Wayland host successfully!\n");

    // 1. Get registry: wl_display.get_registry(registry_id = 2)
    uint32_t reg_arg = 2;
    wl_send_raw(sock, 1, 1, &reg_arg, 4);

    // 2. Sync callback: wl_display.sync(callback_id = next_id)
    uint32_t sync_cb = next_id++;
    wl_send_raw(sock, 1, 0, &sync_cb, 4);

    // Read until sync callback triggers
    uint8_t rx_buf[8192];
    int synced = 0;
    while (!synced) {
        ssize_t n = recv(sock, rx_buf, sizeof(rx_buf), 0);
        if (n <= 0) {
            fprintf(stderr, "recv failed: n=%d, errno=%d (%s)\n", (int)n, errno, strerror(errno));
            return 1;
        }
        printf("Received %zd bytes from Wayland host\n", n);

        size_t offset = 0;
        while (offset + sizeof(struct wl_msg_hdr) <= (size_t)n) {
            struct wl_msg_hdr* h = (struct wl_msg_hdr*)(rx_buf + offset);
            if (h->size == 0 || offset + h->size > (size_t)n) break;
            const uint8_t* payload = rx_buf + offset + sizeof(struct wl_msg_hdr);
            uint16_t plen = h->size - sizeof(struct wl_msg_hdr);

            if (h->id == 2 && h->opcode == 0) {
                parse_registry_global(sock, payload, plen);
            } else if (h->id == sync_cb && h->opcode == 0) {
                printf("Initial sync complete!\n");
                synced = 1;
            } else if (h->id == 1 && h->opcode == 0) {
                // wl_display.error(object_id, code, message)
                uint32_t err_obj = *(uint32_t*)payload;
                uint32_t err_code = *(uint32_t*)(payload + 4);
                uint32_t msg_len = *(uint32_t*)(payload + 8);
                const char* msg = (const char*)(payload + 12);
                fprintf(stderr, "WAYLAND PROTOCOL ERROR: obj=%u code=%u msg=%.*s\n",
                        err_obj, err_code, msg_len, msg);
            }
            offset += h->size;
        }
    }


    if (!compositor_id || !xdg_wm_base_id || !android_wlegl_id) {
        fprintf(stderr, "Missing required globals: comp=%u xdg=%u wlegl=%u\n",
                compositor_id, xdg_wm_base_id, android_wlegl_id);
        return 1;
    }
    printf("All required globals present! comp=%u xdg=%u wlegl=%u\n",
           compositor_id, xdg_wm_base_id, android_wlegl_id);

    // Create EGL context on Qualcomm Adreno 830
    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    eglInitialize(dpy, NULL, NULL);

    EGLint cfg_attribs[] = {
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
        EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
        EGL_RED_SIZE, 8,
        EGL_GREEN_SIZE, 8,
        EGL_BLUE_SIZE, 8,
        EGL_ALPHA_SIZE, 8,
        EGL_NONE
    };
    EGLConfig config;
    EGLint num_configs;
    eglChooseConfig(dpy, cfg_attribs, &config, 1, &num_configs);

    EGLint ctx_attribs[] = { EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, config, EGL_NO_CONTEXT, ctx_attribs);

    EGLint pb_attribs[] = { EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE };
    EGLSurface pbuf = eglCreatePbufferSurface(dpy, config, pb_attribs);
    eglMakeCurrent(dpy, pbuf, pbuf, ctx);

    printf("EGL/GLES Initialized on Adreno!\n");
    printf("GL_RENDERER: %s\n", glGetString(GL_RENDERER));
    printf("GL_VERSION: %s\n", glGetString(GL_VERSION));

    peglGetNativeClientBufferANDROID = (PFNEGLGETNATIVECLIENTBUFFERANDROIDPROC)eglGetProcAddress("eglGetNativeClientBufferANDROID");
    peglCreateImageKHR = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    peglDestroyImageKHR = (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
    pglEGLImageTargetTexture2DOES = (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");

    // Build shader program
    GLuint vs = compile_shader(GL_VERTEX_SHADER, VERTEX_SHADER);
    GLuint fs = compile_shader(GL_FRAGMENT_SHADER, FRAGMENT_SHADER);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glLinkProgram(prog);
    glUseProgram(prog);
    GLint uAngleLoc = glGetUniformLocation(prog, "uAngle");

    // Vertex data
    float vertices[] = {
        // x, y,       r, g, b
         0.0f,  0.7f,  1.0f, 0.0f, 0.0f, // Red
        -0.7f, -0.7f,  0.0f, 1.0f, 0.0f, // Green
         0.7f, -0.7f,  0.0f, 0.0f, 1.0f  // Blue
    };
    GLuint vbo;
    glGenBuffers(1, &vbo);
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STATIC_DRAW);

    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)0);
    glEnableVertexAttribArray(1);
    glVertexAttribPointer(1, 3, GL_FLOAT, GL_FALSE, 5 * sizeof(float), (void*)(2 * sizeof(float)));

    // Create Wayland surface & xdg_toplevel
    uint32_t surface_id = next_id++;
    wl_send_raw(sock, compositor_id, 0, &surface_id, 4); // wl_compositor.create_surface
    printf("Created wl_surface id %u\n", surface_id);

    uint32_t xdg_surface_id = next_id++;
    uint32_t xdg_s_args[2] = { xdg_surface_id, surface_id };
    wl_send_raw(sock, xdg_wm_base_id, 2, xdg_s_args, 8); // xdg_wm_base.get_xdg_surface

    uint32_t toplevel_id = next_id++;
    wl_send_raw(sock, xdg_surface_id, 1, &toplevel_id, 4); // xdg_surface.get_toplevel

    // Set title
    const char* title = "GPU Acceleration Proof (Adreno 830 Hardware)";
    uint32_t tlen = strlen(title) + 1;
    uint32_t title_buf[64];
    title_buf[0] = tlen;
    memcpy(&title_buf[1], title, tlen);
    wl_send_raw(sock, toplevel_id, 2, title_buf, 4 + ((tlen + 3) & ~3));

    // Commit empty surface to get initial configure
    wl_send_raw(sock, surface_id, 6, NULL, 0); // wl_surface.commit
    printf("Committed initial surface, waiting for xdg_surface.configure...\n");

    // Allocate triple-buffered AHardwareBuffers and create wayland buffers
    struct BufferSlot slots[NUM_BUFFERS];
    for (int i = 0; i < NUM_BUFFERS; i++) {
        slots[i].busy = 0;

        AHardwareBuffer_Desc desc = {
            .width = win_w,
            .height = win_h,
            .layers = 1,
            .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
            .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
                     AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
                     AHARDWAREBUFFER_USAGE_CPU_READ_NEVER |
                     AHARDWAREBUFFER_USAGE_CPU_WRITE_NEVER,
            .stride = 0,
            .rfu0 = 0,
            .rfu1 = 0,
        };

        if (AHardwareBuffer_allocate(&desc, &slots[i].ahb) != 0) {
            fprintf(stderr, "AHardwareBuffer_allocate failed for slot %d\n", i);
            return 1;
        }

        AHardwareBuffer_describe(slots[i].ahb, &desc);
        printf("Slot %d allocated AHB: %ux%u stride=%u format=0x%x\n",
               i, desc.width, desc.height, desc.stride, desc.format);

        const native_handle_t* nh = AHardwareBuffer_getNativeHandle(slots[i].ahb);

        // 1. Create android_wlegl_handle
        uint32_t handle_id = next_id++;
        uint32_t num_fds = nh->numFds;
        uint32_t num_ints = nh->numInts;
        uint32_t ints_bytes = num_ints * sizeof(int);

        // android_wlegl.create_handle(id, num_fds, ints)
        uint32_t handle_req[256];
        int hidx = 0;
        handle_req[hidx++] = handle_id;
        handle_req[hidx++] = num_fds;
        handle_req[hidx++] = ints_bytes;
        memcpy(&handle_req[hidx], &nh->data[num_fds], ints_bytes);
        hidx += (ints_bytes + 3) / 4;
        wl_send_raw(sock, android_wlegl_id, 0, handle_req, hidx * 4);

        // 2. Add each FD to handle
        for (uint32_t f = 0; f < num_fds; f++) {
            int fd_copy = dup(nh->data[f]);
            wl_send_fd(sock, handle_id, 0, fd_copy);
            close(fd_copy);
        }

        // 3. Create wl_buffer: android_wlegl.create_buffer
        slots[i].wl_buffer_id = next_id++;
        uint32_t buf_args[7] = {
            slots[i].wl_buffer_id,
            desc.width, desc.height,
            desc.stride, // real gralloc stride!
            desc.format, // format
            (uint32_t)desc.usage,
            handle_id
        };
        wl_send_raw(sock, android_wlegl_id, 1, buf_args, sizeof(buf_args));


        // 4. Destroy handle
        wl_send_raw(sock, handle_id, 1, NULL, 0);

        // 5. Setup GLES FBO targeting this AHB
        EGLClientBuffer cb = peglGetNativeClientBufferANDROID(slots[i].ahb);
        EGLint img_attribs[] = { EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE };
        EGLImageKHR img = peglCreateImageKHR(dpy, EGL_NO_CONTEXT, EGL_NATIVE_BUFFER_ANDROID, cb, img_attribs);

        glGenTextures(1, &slots[i].tex);
        glBindTexture(GL_TEXTURE_2D, slots[i].tex);
        pglEGLImageTargetTexture2DOES(GL_TEXTURE_2D, img);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);

        glGenFramebuffers(1, &slots[i].fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, slots[i].fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, slots[i].tex, 0);

        GLenum status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
        if (status != GL_FRAMEBUFFER_COMPLETE) {
            fprintf(stderr, "FBO incomplete: 0x%X\n", status);
            return 1;
        }
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        peglDestroyImageKHR(dpy, img);

        printf("Slot %d: AHB allocated, wlegl buffer %u, GLES FBO %u complete!\n",
               i, slots[i].wl_buffer_id, slots[i].fbo);
    }

    // Set non-blocking socket to process server messages asynchronously
    int flags = fcntl(sock, F_GETFL, 0);
    fcntl(sock, F_SETFL, flags | O_NONBLOCK);

    printf("\n=== Rendering %d Hardware-Accelerated Frames into Smithay ===\n", frame_limit);
    int frame_count = 0;
    int cur_slot = 0;
    unsigned int release_count = 0;

    struct timespec t_start, t_end;
    clock_gettime(CLOCK_MONOTONIC, &t_start);

    while (frame_count < frame_limit) {

        // Read any pending Wayland messages (ping, configure, buffer.release)
        ssize_t n = recv(sock, rx_buf, sizeof(rx_buf), 0);
        if (n > 0) {
            size_t offset = 0;
            while (offset + sizeof(struct wl_msg_hdr) <= (size_t)n) {
                struct wl_msg_hdr* h = (struct wl_msg_hdr*)(rx_buf + offset);
                if (h->size == 0 || offset + h->size > (size_t)n) break;
                const uint8_t* payload = rx_buf + offset + sizeof(struct wl_msg_hdr);

                // xdg_wm_base.ping
                if (h->id == xdg_wm_base_id && h->opcode == 0) {
                    uint32_t serial = *(uint32_t*)payload;
                    wl_send_raw(sock, xdg_wm_base_id, 3, &serial, 4); // pong
                }
                // xdg_surface.configure
                else if (h->id == xdg_surface_id && h->opcode == 0) {
                    uint32_t serial = *(uint32_t*)payload;
                    wl_send_raw(sock, xdg_surface_id, 4, &serial, 4); // ack_configure
                    printf("Received and ACKed xdg_surface.configure serial=%u\n", serial);
                }
                // wl_buffer.release
                else {
                    for (int s = 0; s < NUM_BUFFERS; s++) {
                        if (h->id == slots[s].wl_buffer_id && h->opcode == 0) {
                            slots[s].busy = 0;
                            release_count++;
                            if (frame_count < 10) {
                                printf("wl_buffer.release received for buffer id=%u\n", slots[s].wl_buffer_id);
                            }
                        }
                    }
                }

                offset += h->size;
            }
        }

        // Find an available slot
        if (slots[cur_slot].busy) {
            usleep(1000);
            continue;
        }

        struct BufferSlot* slot = &slots[cur_slot];

        // Hardware GLES rendering into AHB FBO
        glBindFramebuffer(GL_FRAMEBUFFER, slot->fbo);
        glViewport(0, 0, win_w, win_h);

        // Animated background color
        float phase = (float)frame_count * 0.05f;
        float bg_r = 0.1f + 0.1f * sinf(phase);
        float bg_g = 0.1f + 0.1f * cosf(phase);
        float bg_b = 0.25f;
        glClearColor(bg_r, bg_g, bg_b, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);

        // Draw spinning triangle
        glUseProgram(prog);
        glUniform1f(uAngleLoc, phase * 2.0f);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glDrawArrays(GL_TRIANGLES, 0, 3);
        glFinish(); // Ensure Adreno hardware commands complete

        glBindFramebuffer(GL_FRAMEBUFFER, 0);

        // Attach buffer to wl_surface
        uint32_t attach_args[3] = { slot->wl_buffer_id, 0, 0 };
        wl_send_raw(sock, surface_id, 1, attach_args, 12); // wl_surface.attach

        // Damage entire surface
        uint32_t damage_args[4] = { 0, 0, win_w, win_h };
        wl_send_raw(sock, surface_id, 2, damage_args, 16); // wl_surface.damage

        // Commit surface
        wl_send_raw(sock, surface_id, 6, NULL, 0); // wl_surface.commit

        slot->busy = 1;
        cur_slot = (cur_slot + 1) % NUM_BUFFERS;
        frame_count++;

        if (frame_count % 30 == 0) {
            printf("Rendered and committed frame %d/%d...\n", frame_count, frame_limit);
        }

        usleep(16000); // Target ~60 FPS
    }


    clock_gettime(CLOCK_MONOTONIC, &t_end);
    double elapsed = (t_end.tv_sec - t_start.tv_sec) + (t_end.tv_nsec - t_start.tv_nsec) / 1e9;
    printf("\n=== SUCCESS: %d Hardware-Accelerated Frames Completed in %.2f s (%.1f FPS) ===\n",
           frame_count, elapsed, frame_count / elapsed);

    // Keep displaying so screenshots/lifecycle checks can observe the final frame.
    printf("Keeping window alive for %d seconds...\n", hold_seconds);
    sleep(hold_seconds);

    // Detach the final buffer so the compositor can release every slot before cleanup.
    uint32_t detach_args[3] = { 0, 0, 0 };
    wl_send_raw(sock, surface_id, 1, detach_args, sizeof(detach_args));
    wl_send_raw(sock, surface_id, 6, NULL, 0);

    struct timespec release_deadline;
    clock_gettime(CLOCK_MONOTONIC, &release_deadline);
    release_deadline.tv_sec += 3;
    for (;;) {
        int busy = 0;
        for (int s = 0; s < NUM_BUFFERS; s++) busy += slots[s].busy != 0;
        if (busy == 0) break;

        ssize_t n = recv(sock, rx_buf, sizeof(rx_buf), 0);
        if (n > 0) {
            size_t offset = 0;
            while (offset + sizeof(struct wl_msg_hdr) <= (size_t)n) {
                struct wl_msg_hdr* h = (struct wl_msg_hdr*)(rx_buf + offset);
                if (h->size == 0 || offset + h->size > (size_t)n) break;
                for (int s = 0; s < NUM_BUFFERS; s++) {
                    if (h->id == slots[s].wl_buffer_id && h->opcode == 0 && slots[s].busy) {
                        slots[s].busy = 0;
                        release_count++;
                    }
                }
                offset += h->size;
            }
        }

        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec > release_deadline.tv_sec ||
            (now.tv_sec == release_deadline.tv_sec && now.tv_nsec >= release_deadline.tv_nsec)) {
            break;
        }
        usleep(1000);
    }

    int busy_after_detach = 0;
    for (int s = 0; s < NUM_BUFFERS; s++) busy_after_detach += slots[s].busy != 0;
    printf("Buffer lifecycle: releases=%u busy_after_detach=%d\n",
           release_count, busy_after_detach);

    for (int i = 0; i < NUM_BUFFERS; i++) {
        wl_send_raw(sock, slots[i].wl_buffer_id, 0, NULL, 0);
        glDeleteFramebuffers(1, &slots[i].fbo);
        glDeleteTextures(1, &slots[i].tex);
        AHardwareBuffer_release(slots[i].ahb);
    }
    glDeleteProgram(prog);
    glDeleteShader(vs);
    glDeleteShader(fs);
    glDeleteBuffers(1, &vbo);

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroySurface(dpy, pbuf);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);

    close(sock);
    return busy_after_detach == 0 ? 0 : 3;
}
