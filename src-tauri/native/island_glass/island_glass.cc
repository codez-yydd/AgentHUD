#include "island_glass.h"
#include "lens_shader.h"

#include <windows.h>
#include <d3d11.h>
#include <d3dcompiler.h>
#include <dcomp.h>
#include <dwmapi.h>
#include <dxgi1_5.h>
#include <wrl/client.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <iterator>
#include <memory>
#include <mutex>
#include <optional>
#include <thread>
#include <vector>

namespace {
using Microsoft::WRL::ComPtr;
using namespace std::chrono_literals;
using Clock = std::chrono::steady_clock;

constexpr wchar_t kWindowClass[] = L"AgentHudIslandGlassCompanion";
constexpr auto kFormat = DXGI_FORMAT_B8G8R8A8_UNORM;
constexpr float kMargin = 40.0f;
constexpr float kBlurStep = 2.0f;
constexpr auto kAcquireTimeoutMs = 8;
constexpr int kLumaSide = 4;

int width(const RECT& r) { return r.right - r.left; }
int height(const RECT& r) { return r.bottom - r.top; }

bool near_rect(const RECT& left, const RECT& right) {
    constexpr int tolerance = 4;
    return !IsRectEmpty(&right) &&
           std::abs(left.left - right.left) <= tolerance && std::abs(left.top - right.top) <= tolerance &&
           std::abs(left.right - right.right) <= tolerance && std::abs(left.bottom - right.bottom) <= tolerance;
}

bool anchor_excluded(HWND anchor) {
    DWORD affinity = 0;
    return GetWindowDisplayAffinity(anchor, &affinity) && affinity == WDA_EXCLUDEFROMCAPTURE;
}

struct Graphics {
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    DXGI_OUTPUT_DESC output_desc{};
    ComPtr<IDXGIOutput> output;

    bool initialize(HMONITOR monitor) {
        ComPtr<IDXGIFactory1> factory;
        if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) return false;
        for (UINT i = 0;; ++i) {
            ComPtr<IDXGIAdapter1> adapter;
            if (FAILED(factory->EnumAdapters1(i, &adapter))) return false;
            for (UINT j = 0;; ++j) {
                ComPtr<IDXGIOutput> candidate;
                if (FAILED(adapter->EnumOutputs(j, &candidate))) break;
                DXGI_OUTPUT_DESC desc{};
                if (FAILED(candidate->GetDesc(&desc)) || !desc.AttachedToDesktop || desc.Monitor != monitor) continue;
                if (FAILED(D3D11CreateDevice(adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
                                             D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0,
                                             D3D11_SDK_VERSION, &device, nullptr, &context))) return false;
                output_desc = desc;
                output = std::move(candidate);
                return true;
            }
        }
    }
};

class DesktopCapture {
public:
    explicit DesktopCapture(Graphics& graphics) : graphics_(graphics) {}

    bool open() {
        if (duplication_) return true;
        ComPtr<IDXGIOutput5> output5;
        if (SUCCEEDED(graphics_.output.As(&output5)) &&
            SUCCEEDED(output5->DuplicateOutput1(graphics_.device.Get(), 0, 1, &kFormat, &duplication_))) {
            reset_ = true;
            return true;
        }
        ComPtr<IDXGIOutput1> output1;
        reset_ = true;
        return SUCCEEDED(graphics_.output.As(&output1)) &&
               SUCCEEDED(output1->DuplicateOutput(graphics_.device.Get(), &duplication_));
    }

    void close() { duplication_.Reset(); }

    ID3D11ShaderResourceView* view() const { return reset_ ? nullptr : view_.Get(); }

    bool update(const RECT& anchor, const RECT& companion, RECT& changed) {
        changed = {};
        DXGI_OUTDUPL_FRAME_INFO info{};
        ComPtr<IDXGIResource> resource;
        const HRESULT hr = duplication_->AcquireNextFrame(kAcquireTimeoutMs, &info, &resource);
        if (hr == DXGI_ERROR_WAIT_TIMEOUT) return true;
        if (FAILED(hr)) return false;

        // Some drivers return an empty surface for the first pointer-only
        // event. Wait for a real desktop update before exposing the lens.
        const bool pointer_only = info.LastPresentTime.QuadPart == 0 && info.AccumulatedFrames == 0 &&
                                  info.TotalMetadataBufferSize == 0;
        const bool copied = pointer_only || copy_frame(resource.Get(), info, anchor, companion, changed);
        return SUCCEEDED(duplication_->ReleaseFrame()) && copied;
    }

private:
    RECT screen_rect(const RECT& raw) const {
        const auto& output = graphics_.output_desc;
        const RECT desktop = output.DesktopCoordinates;
        RECT result = raw;
        switch (output.Rotation) {
        case DXGI_MODE_ROTATION_ROTATE90:
            result = {width(desktop) - raw.bottom, raw.left, width(desktop) - raw.top, raw.right};
            break;
        case DXGI_MODE_ROTATION_ROTATE180:
            result = {width(desktop) - raw.right, height(desktop) - raw.bottom,
                      width(desktop) - raw.left, height(desktop) - raw.top};
            break;
        case DXGI_MODE_ROTATION_ROTATE270:
            result = {raw.top, height(desktop) - raw.right, raw.bottom, height(desktop) - raw.left};
            break;
        default:
            break;
        }
        OffsetRect(&result, desktop.left, desktop.top);
        return result;
    }

    // The current DDA surface is complete. Move destinations can therefore be
    // copied just like ordinary dirt, without applying moves to the mirror.
    RECT damage(const DXGI_OUTDUPL_FRAME_INFO& info, const RECT& anchor, const RECT& companion,
                const RECT& limits) {
        if (!info.TotalMetadataBufferSize) return limits;
        metadata_.resize((info.TotalMetadataBufferSize + sizeof(RECT) - 1) / sizeof(RECT));
        const UINT bytes = static_cast<UINT>(metadata_.size() * sizeof(RECT));
        UINT used = 0;
        RECT changed{};
        if (FAILED(duplication_->GetFrameMoveRects(bytes,
            reinterpret_cast<DXGI_OUTDUPL_MOVE_RECT*>(metadata_.data()), &used))) {
            return limits;
        }
        const auto* moves = reinterpret_cast<DXGI_OUTDUPL_MOVE_RECT*>(metadata_.data());
        for (UINT i = 0; i < used / sizeof(*moves); ++i) {
            UnionRect(&changed, &changed, &moves[i].DestinationRect);
        }

        if (FAILED(duplication_->GetFrameDirtyRects(bytes, metadata_.data(), &used))) {
            return limits;
        }
        for (UINT i = 0; i < used / sizeof(RECT); ++i) {
            const RECT& dirty = metadata_[i];
            const RECT screen = screen_rect(dirty);
            // DWM can report these windows' presents as desktop damage even
            // though display affinity excludes their pixels from the frame.
            if (!near_rect(screen, anchor) && !near_rect(screen, companion)) {
                UnionRect(&changed, &changed, &dirty);
            }
        }
        return changed;
    }

    bool copy_frame(IDXGIResource* resource, const DXGI_OUTDUPL_FRAME_INFO& info,
                    const RECT& anchor, const RECT& companion, RECT& changed) {
        ComPtr<ID3D11Texture2D> frame;
        if (!resource || FAILED(resource->QueryInterface(IID_PPV_ARGS(&frame)))) return false;
        D3D11_TEXTURE2D_DESC desc{};
        frame->GetDesc(&desc);
        const RECT desktop = graphics_.output_desc.DesktopCoordinates;
        const auto rotation = graphics_.output_desc.Rotation;
        const bool swapped = rotation == DXGI_MODE_ROTATION_ROTATE90 || rotation == DXGI_MODE_ROTATION_ROTATE270;
        const bool supported_format = desc.Format == kFormat || desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM ||
                                      desc.Format == DXGI_FORMAT_R10G10B10A2_UNORM;
        if (!supported_format || desc.Width != static_cast<UINT>(swapped ? height(desktop) : width(desktop)) ||
            desc.Height != static_cast<UINT>(swapped ? width(desktop) : height(desktop))) return false;

        if (!mirror_ || desc.Format != mirror_format_) {
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
            desc.CPUAccessFlags = 0;
            desc.MiscFlags = 0;
            if (FAILED(graphics_.device->CreateTexture2D(&desc, nullptr, mirror_.ReleaseAndGetAddressOf())) ||
                FAILED(graphics_.device->CreateShaderResourceView(mirror_.Get(), nullptr, view_.ReleaseAndGetAddressOf()))) return false;
            mirror_format_ = desc.Format;
            reset_ = true;
        }

        if (reset_) {
            reset_ = false;
            graphics_.context->CopyResource(mirror_.Get(), frame.Get());
            changed = desktop;
            return true;
        }

        const RECT limits{0, 0, static_cast<LONG>(desc.Width), static_cast<LONG>(desc.Height)};
        const RECT dirty = damage(info, anchor, companion, limits);
        RECT clipped{};
        if (!IntersectRect(&clipped, &dirty, &limits)) return true;
        const D3D11_BOX box{static_cast<UINT>(clipped.left), static_cast<UINT>(clipped.top), 0,
                            static_cast<UINT>(clipped.right), static_cast<UINT>(clipped.bottom), 1};
        graphics_.context->CopySubresourceRegion(mirror_.Get(), 0, box.left, box.top, 0, frame.Get(), 0, &box);
        changed = screen_rect(clipped);
        return true;
    }

    Graphics& graphics_;
    bool reset_ = true;
    DXGI_FORMAT mirror_format_ = DXGI_FORMAT_UNKNOWN;
    std::vector<RECT> metadata_;
    ComPtr<IDXGIOutputDuplication> duplication_;
    ComPtr<ID3D11Texture2D> mirror_;
    ComPtr<ID3D11ShaderResourceView> view_;
};

LRESULT CALLBACK companion_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message == WM_NCHITTEST) return HTTRANSPARENT;
    if (message == WM_MOUSEACTIVATE) return MA_NOACTIVATE;
    return DefWindowProcW(window, message, wparam, lparam);
}

bool register_companion_class() {
    static const bool registered = [] {
        WNDCLASSW window_class{};
        window_class.lpfnWndProc = companion_proc;
        window_class.hInstance = GetModuleHandleW(nullptr);
        window_class.lpszClassName = kWindowClass;
        return RegisterClassW(&window_class) != 0;
    }();
    return registered;
}

class Companion {
public:
    ~Companion() {
        if (window_) DestroyWindow(window_);
    }

    bool initialize(ID3D11Device* device, HWND anchor) {
        if (!register_companion_class()) return false;
        // Layered + transparent lets input pass through to other threads while
        // DirectComposition supplies content without a redirection bitmap.
        DWORD styles = WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
                       WS_EX_LAYERED | WS_EX_TRANSPARENT;
        if (is_topmost(anchor)) styles |= WS_EX_TOPMOST;
        window_ = CreateWindowExW(styles,
                                  kWindowClass, L"", WS_POPUP | WS_DISABLED,
                                  0, 0, 0, 0, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
        if (!window_ || !SetLayeredWindowAttributes(window_, 0, 255, LWA_ALPHA) ||
            !SetWindowDisplayAffinity(window_, WDA_EXCLUDEFROMCAPTURE)) return false;

        ComPtr<IDXGIDevice> dxgi;
        ComPtr<IDXGIAdapter> adapter;
        ComPtr<IDXGIFactory2> factory;
        if (FAILED(device->QueryInterface(IID_PPV_ARGS(&dxgi))) || FAILED(dxgi->GetAdapter(&adapter)) ||
            FAILED(adapter->GetParent(IID_PPV_ARGS(&factory)))) return false;

        DXGI_SWAP_CHAIN_DESC1 desc{};
        desc.Width = 1;
        desc.Height = 1;
        desc.Format = kFormat;
        desc.SampleDesc.Count = 1;
        desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        desc.BufferCount = 2;
        desc.Scaling = DXGI_SCALING_STRETCH;
        desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL;
        desc.AlphaMode = DXGI_ALPHA_MODE_PREMULTIPLIED;
        return SUCCEEDED(factory->CreateSwapChainForComposition(device, &desc, nullptr, &swapchain_)) &&
               SUCCEEDED(DCompositionCreateDevice(dxgi.Get(), IID_PPV_ARGS(&composition_))) &&
               SUCCEEDED(composition_->CreateTargetForHwnd(window_, TRUE, &target_)) &&
               SUCCEEDED(composition_->CreateVisual(&visual_)) &&
               SUCCEEDED(visual_->SetContent(swapchain_.Get())) &&
               SUCCEEDED(target_->SetRoot(visual_.Get())) && SUCCEEDED(composition_->Commit());
    }

    bool place(HWND anchor, const RECT& bounds) {
        constexpr UINT position_flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        const auto now = Clock::now();
        const bool topmost = is_topmost(anchor);
        const bool changed_tier = topmost != is_topmost(window_);
        if (changed_tier && !SetWindowPos(window_, topmost ? HWND_TOPMOST : HWND_NOTOPMOST,
                                         0, 0, 0, 0, position_flags | SWP_NOMOVE | SWP_NOSIZE)) return false;

        bool restack = !visible_ || changed_tier;
        if (restack || now >= next_stack_check_) {
            next_stack_check_ = now + 250ms;
            auto below = GetWindow(anchor, GW_HWNDNEXT);
            while (below && !IsWindowVisible(below)) below = GetWindow(below, GW_HWNDNEXT);
            restack = restack || below != window_;
        }
        if (restack || !EqualRect(&bounds_, &bounds)) {
            const UINT flags = position_flags | SWP_SHOWWINDOW | (restack ? 0 : SWP_NOZORDER);
            if (!SetWindowPos(window_, anchor, bounds.left, bounds.top, width(bounds), height(bounds), flags)) return false;
            visible_ = true;
            bounds_ = bounds;
        }
        return true;
    }

    bool present() { return SUCCEEDED(swapchain_->Present(0, 0)); }

    void hide() {
        if (visible_) ShowWindow(window_, SW_HIDE);
        visible_ = false;
    }

    const RECT& bounds() const { return bounds_; }

    bool acquire_buffer(ID3D11Device* device, int w, int h, ID3D11RenderTargetView** view) {
        if (w != buffer_width_ || h != buffer_height_) {
            if (FAILED(swapchain_->ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, 0))) return false;
            buffer_width_ = w;
            buffer_height_ = h;
        }
        ComPtr<ID3D11Texture2D> buffer;
        return SUCCEEDED(swapchain_->GetBuffer(0, IID_PPV_ARGS(&buffer))) &&
               SUCCEEDED(device->CreateRenderTargetView(buffer.Get(), nullptr, view));
    }

private:
    static bool is_topmost(HWND window) {
        return (GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_TOPMOST) != 0;
    }

    HWND window_ = nullptr;
    bool visible_ = false;
    RECT bounds_{};
    Clock::time_point next_stack_check_{};
    int buffer_width_ = 1;
    int buffer_height_ = 1;
    ComPtr<IDXGISwapChain1> swapchain_;
    ComPtr<IDCompositionDevice> composition_;
    ComPtr<IDCompositionTarget> target_;
    ComPtr<IDCompositionVisual> visual_;
};

struct PhysicalSurface {
    RECT bounds{};
    RECT clip{};
    float scale = 1.0f;
    float radii[4]{};

    bool matches(const PhysicalSurface& other) const {
        return EqualRect(&bounds, &other.bounds) && EqualRect(&clip, &other.clip) && scale == other.scale &&
               std::equal(std::begin(radii), std::end(radii), std::begin(other.radii));
    }
};

std::optional<PhysicalSurface> locate_surface(HWND anchor, const AgentHudGlassSurface& input) {
    RECT client{};
    POINT origin{};
    if (!GetClientRect(anchor, &client) || !ClientToScreen(anchor, &origin)) return std::nullopt;
    const UINT dpi = GetDpiForWindow(anchor);
    if (dpi == 0) return std::nullopt;
    const float scale = dpi / 96.0f;

    PhysicalSurface surface;
    surface.scale = scale;
    surface.bounds = {origin.x + std::lround(input.x * scale),
                      origin.y + std::lround(input.y * scale),
                      origin.x + std::lround((input.x + input.width) * scale),
                      origin.y + std::lround((input.y + input.height) * scale)};
    if (IsRectEmpty(&surface.bounds)) return std::nullopt;
    OffsetRect(&client, origin.x, origin.y);
    if (!IntersectRect(&surface.clip, &client, &surface.bounds)) return std::nullopt;
    const float padded_width = width(surface.bounds) + 2.0f * kMargin * scale;
    const float padded_height = height(surface.bounds) + 2.0f * kMargin * scale;
    if (padded_width > D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION ||
        padded_height > D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION) return std::nullopt;
    for (int i = 0; i < 4; ++i) surface.radii[i] = input.radii[i] * scale;
    return surface;
}

struct ShaderParams {
    float origin[2]{};
    float span[2]{};
    float output[2]{};
    float direction[2]{};
    float glass[2]{};
    float bezel = 0.0f;
    float bend = 0.0f;
    float margin = 0.0f;
    float rotation = 0.0f;
    float saturation = 1.15f;
    float aberration = 1.3f;
    float radii[4]{};
    float clip[4]{};
};
static_assert(sizeof(ShaderParams) == 6 * 16);

bool compile(const char* entry, const char* model, ID3DBlob** code) {
    ComPtr<ID3DBlob> error;
    const HRESULT hr = D3DCompile(kGlassShader, sizeof(kGlassShader) - 1, "island-glass.hlsl", nullptr, nullptr,
                                  entry, model, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, code, &error);
    if (FAILED(hr) && error) {
        OutputDebugStringA(static_cast<const char*>(error->GetBufferPointer()));
    }
    return SUCCEEDED(hr);
}

struct RenderTexture {
    ComPtr<ID3D11Texture2D> texture;
    ComPtr<ID3D11RenderTargetView> target;
    ComPtr<ID3D11ShaderResourceView> view;

    bool create(ID3D11Device* device, UINT w, UINT h) {
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width = w;
        desc.Height = h;
        desc.MipLevels = 1;
        desc.ArraySize = 1;
        desc.Format = kFormat;
        desc.SampleDesc.Count = 1;
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
        return SUCCEEDED(device->CreateTexture2D(&desc, nullptr, texture.ReleaseAndGetAddressOf())) &&
               SUCCEEDED(device->CreateRenderTargetView(texture.Get(), nullptr, target.ReleaseAndGetAddressOf())) &&
               SUCCEEDED(device->CreateShaderResourceView(texture.Get(), nullptr, view.ReleaseAndGetAddressOf()));
    }
};

class LensRenderer {
public:
    explicit LensRenderer(Graphics& graphics) : graphics_(graphics) {}

    bool initialize() {
        const auto& device = graphics_.device;
        ComPtr<ID3DBlob> bytecode;
        if (!compile("VSMain", "vs_5_0", &bytecode) ||
            FAILED(device->CreateVertexShader(bytecode->GetBufferPointer(), bytecode->GetBufferSize(), nullptr, &vertex_))) return false;
        if (!compile("PSBlur", "ps_5_0", bytecode.ReleaseAndGetAddressOf()) ||
            FAILED(device->CreatePixelShader(bytecode->GetBufferPointer(), bytecode->GetBufferSize(), nullptr, &blur_))) return false;
        if (!compile("PSLens", "ps_5_0", bytecode.ReleaseAndGetAddressOf()) ||
            FAILED(device->CreatePixelShader(bytecode->GetBufferPointer(), bytecode->GetBufferSize(), nullptr, &lens_))) return false;

        D3D11_SAMPLER_DESC sample{};
        sample.Filter = D3D11_FILTER_MIN_MAG_MIP_LINEAR;
        sample.AddressU = sample.AddressV = sample.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
        sample.MaxAnisotropy = 1;
        sample.ComparisonFunc = D3D11_COMPARISON_NEVER;
        const D3D11_BUFFER_DESC constants{sizeof(ShaderParams), D3D11_USAGE_DEFAULT, D3D11_BIND_CONSTANT_BUFFER, 0, 0, 0};
        return SUCCEEDED(device->CreateSamplerState(&sample, &sampler_)) &&
               SUCCEEDED(device->CreateBuffer(&constants, nullptr, &constants_));
    }

    bool render(ID3D11ShaderResourceView* desktop, const PhysicalSurface& surface, Companion& companion) {
        const auto& ctx = graphics_.context;
        const RECT& output = graphics_.output_desc.DesktopCoordinates;
        const int w = width(surface.bounds);
        const int h = height(surface.bounds);
        const float margin = std::round(kMargin * surface.scale);
        const float region_w = w + 2.0f * margin;
        const float region_h = h + 2.0f * margin;
        const UINT blur_w = (static_cast<UINT>(region_w) + 1) / 2;
        const UINT blur_h = (static_cast<UINT>(region_h) + 1) / 2;
        if (!ensure_blur(blur_w, blur_h)) return false;

        ctx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        ctx->VSSetShader(vertex_.Get(), nullptr, 0);
        ctx->PSSetSamplers(0, 1, sampler_.GetAddressOf());
        ctx->PSSetConstantBuffers(0, 1, constants_.GetAddressOf());

        ShaderParams horizontal;
        horizontal.origin[0] = (surface.bounds.left - output.left - margin) / width(output);
        horizontal.origin[1] = (surface.bounds.top - output.top - margin) / height(output);
        horizontal.span[0] = region_w / width(output);
        horizontal.span[1] = region_h / height(output);
        horizontal.direction[0] = kBlurStep * surface.scale / width(output);
        horizontal.rotation = static_cast<float>(graphics_.output_desc.Rotation);
        draw(blur_.Get(), desktop, horizontal_.target.Get(), horizontal, blur_w, blur_h);

        ShaderParams vertical;
        vertical.span[0] = vertical.span[1] = 1.0f;
        vertical.direction[1] = kBlurStep * surface.scale / region_h;
        draw(blur_.Get(), horizontal_.view.Get(), vertical_.target.Get(), vertical, blur_w, blur_h);

        ShaderParams final = horizontal;
        final.glass[0] = static_cast<float>(w);
        final.glass[1] = static_cast<float>(h);
        final.margin = margin;
        final.bezel = std::min(34.0f * surface.scale, std::min(w, h) * 0.375f);
        // Limits the sampling slope so the edge can stretch and compress
        // background features without reversing the image.
        final.bend = 0.289f * final.bezel;
        std::copy(std::begin(surface.radii), std::end(surface.radii), std::begin(final.radii));
        final.clip[0] = static_cast<float>(surface.clip.left - surface.bounds.left);
        final.clip[1] = static_cast<float>(surface.clip.top - surface.bounds.top);
        final.clip[2] = static_cast<float>(surface.clip.right - surface.bounds.left);
        final.clip[3] = static_cast<float>(surface.clip.bottom - surface.bounds.top);

        ComPtr<ID3D11RenderTargetView> target;
        if (!companion.acquire_buffer(graphics_.device.Get(), w, h, &target)) return false;
        draw(lens_.Get(), vertical_.view.Get(), target.Get(), final, w, h);
        luma_params_.origin[0] = margin / region_w;
        luma_params_.origin[1] = margin / region_h;
        luma_params_.span[0] = w / region_w;
        luma_params_.span[1] = h / region_h;
        luma_dirty_ = true;
        queue_luma();
        return companion.present();
    }

    void sample_static_frame() {
        if (queue_luma()) graphics_.context->Flush();
    }

    void discard_luma() {
        luma_dirty_ = false;
        luma_pending_ = false;
    }

    std::optional<int> read_luma() {
        if (!luma_pending_) return std::nullopt;
        const auto& ctx = graphics_.context;
        D3D11_MAPPED_SUBRESOURCE map{};
        const HRESULT hr = ctx->Map(luma_staging_.Get(), 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT, &map);
        if (hr == DXGI_ERROR_WAS_STILL_DRAWING) return std::nullopt;
        luma_pending_ = false;
        if (FAILED(hr)) return std::nullopt;

        float sum = 0.0f;
        for (int y = 0; y < kLumaSide; ++y) {
            const auto* line = static_cast<const unsigned char*>(map.pData) + y * map.RowPitch;
            for (int x = 0; x < kLumaSide; ++x) {
                const auto* pixel = line + 4 * x;
                sum += 0.2126f * pixel[2] + 0.7152f * pixel[1] + 0.0722f * pixel[0];
            }
        }
        ctx->Unmap(luma_staging_.Get(), 0);
        return static_cast<int>(std::lround(sum / (kLumaSide * kLumaSide)));
    }

private:
    bool ensure_blur(UINT w, UINT h) {
        if (w == blur_width_ && h == blur_height_) return true;
        auto* device = graphics_.device.Get();
        if (!horizontal_.create(device, w, h) || !vertical_.create(device, w, h)) return false;
        blur_width_ = w;
        blur_height_ = h;
        return true;
    }

    void draw(ID3D11PixelShader* shader, ID3D11ShaderResourceView* source, ID3D11RenderTargetView* target,
              ShaderParams& params, UINT w, UINT h) {
        const auto& ctx = graphics_.context;
        params.output[0] = static_cast<float>(w);
        params.output[1] = static_cast<float>(h);
        const D3D11_VIEWPORT viewport{0, 0, params.output[0], params.output[1], 0, 1};
        ctx->UpdateSubresource(constants_.Get(), 0, nullptr, &params, 0, 0);
        ctx->RSSetViewports(1, &viewport);
        ctx->OMSetRenderTargets(1, &target, nullptr);
        ctx->PSSetShader(shader, nullptr, 0);
        ctx->PSSetShaderResources(0, 1, &source);
        ctx->Draw(3, 0);
        ID3D11ShaderResourceView* no_source = nullptr;
        ctx->PSSetShaderResources(0, 1, &no_source);
        ctx->OMSetRenderTargets(0, nullptr, nullptr);
    }

    bool queue_luma() {
        const auto now = Clock::now();
        if (!luma_dirty_ || luma_pending_ || now < next_luma_) return false;
        next_luma_ = now + 150ms;
        const auto& device = graphics_.device;
        if (!luma_staging_) {
            if (!luma_target_.create(device.Get(), kLumaSide, kLumaSide)) return false;
            D3D11_TEXTURE2D_DESC desc{};
            luma_target_.texture->GetDesc(&desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
            if (FAILED(device->CreateTexture2D(&desc, nullptr, &luma_staging_))) return false;
        }
        draw(blur_.Get(), vertical_.view.Get(), luma_target_.target.Get(), luma_params_, kLumaSide, kLumaSide);
        graphics_.context->CopyResource(luma_staging_.Get(), luma_target_.texture.Get());
        luma_dirty_ = false;
        luma_pending_ = true;
        return true;
    }

    Graphics& graphics_;
    RenderTexture horizontal_, vertical_, luma_target_;
    ComPtr<ID3D11Texture2D> luma_staging_;
    ComPtr<ID3D11VertexShader> vertex_;
    ComPtr<ID3D11PixelShader> blur_, lens_;
    ComPtr<ID3D11SamplerState> sampler_;
    ComPtr<ID3D11Buffer> constants_;
    UINT blur_width_ = 0;
    UINT blur_height_ = 0;
    ShaderParams luma_params_{};
    Clock::time_point next_luma_{};
    bool luma_dirty_ = false;
    bool luma_pending_ = false;
};

struct WorkerState {
    explicit WorkerState(HWND window) : anchor(window) {}

    const HWND anchor;
    std::atomic<bool> stop{false};
    std::atomic<bool> paused{false};
    std::atomic<bool> ready{false};
    std::atomic<int> luma{-1};
    std::mutex mutex;
    std::condition_variable wake;
    std::optional<AgentHudGlassSurface> surface;
};

using GlassHandle = std::shared_ptr<WorkerState>;

bool visible_anchor(HWND anchor) {
    if (!IsWindowVisible(anchor) || IsIconic(anchor) || !anchor_excluded(anchor)) return false;
    DWORD cloaked = 0;
    return FAILED(DwmGetWindowAttribute(anchor, DWMWA_CLOAKED, &cloaked, sizeof(cloaked))) || cloaked == 0;
}

std::optional<PhysicalSurface> current_surface(WorkerState& state) {
    std::optional<AgentHudGlassSurface> input;
    {
        const std::lock_guard lock(state.mutex);
        input = state.surface;
    }
    return input && visible_anchor(state.anchor) ? locate_surface(state.anchor, *input) : std::nullopt;
}

bool valid(const AgentHudGlassSurface& surface) {
    const auto coordinate = [](float value) { return std::isfinite(value) && std::abs(value) <= 32767.0f; };
    return coordinate(surface.x) && coordinate(surface.y) && coordinate(surface.width) &&
           coordinate(surface.height) && surface.width > 0.0f && surface.height > 0.0f &&
           std::all_of(std::begin(surface.radii), std::end(surface.radii),
                       [&](float radius) { return coordinate(radius) && radius >= 0.0f; });
}

void pump_messages() {
    MSG message{};
    while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

void wait(WorkerState& state, std::chrono::milliseconds timeout) {
    std::unique_lock lock(state.mutex);
    if (!state.stop) state.wake.wait_for(lock, timeout);
}

struct Pipeline {
    Graphics graphics;
    DesktopCapture capture{graphics};
    Companion companion;
    LensRenderer renderer{graphics};
    std::optional<PhysicalSurface> last_render;

    bool initialize(HMONITOR monitor, HWND anchor) {
        return graphics.initialize(monitor) && companion.initialize(graphics.device.Get(), anchor) && renderer.initialize();
    }

    void hide(WorkerState& state) {
        companion.hide();
        last_render.reset();
        renderer.discard_luma();
        state.ready = false;
        state.luma = -1;
    }
};

void run(WorkerState& state) {
    const HWND anchor = state.anchor;
    HMONITOR current_monitor = nullptr;
    std::unique_ptr<Pipeline> pipeline;
    while (!state.stop && IsWindow(anchor)) {
        pump_messages();
        auto surface = current_surface(state);
        const HMONITOR monitor = surface ? MonitorFromRect(&surface->clip, MONITOR_DEFAULTTONULL) : nullptr;
        if (!monitor) {
            if (pipeline) {
                pipeline->capture.close();
                pipeline->hide(state);
            }
            wait(state, 40ms);
            continue;
        }

        if (monitor != current_monitor) {
            if (pipeline) pipeline->hide(state);
            pipeline.reset();
            current_monitor = monitor;
        }
        if (state.paused) {
            // In-app screenshots/recording need the duplication slot. Keep the
            // visible buffer only as long as its captured screen region fits.
            if (pipeline) {
                pipeline->capture.close();
                if (!pipeline->last_render || !pipeline->last_render->matches(*surface) ||
                    !pipeline->companion.place(anchor, surface->bounds)) pipeline->hide(state);
            }
            wait(state, 25ms);
            continue;
        }

        if (!pipeline) {
            auto candidate = std::make_unique<Pipeline>();
            if (!candidate->initialize(monitor, anchor)) {
                wait(state, 750ms);
                continue;
            }
            pipeline = std::move(candidate);
        }
        if (!pipeline->capture.open()) {
            pipeline->hide(state);
            wait(state, 250ms);
            continue;
        }

        RECT anchor_rect{};
        GetWindowRect(anchor, &anchor_rect);
        RECT dirty{};
        if (!pipeline->capture.update(anchor_rect, pipeline->companion.bounds(), dirty)) {
            pipeline->hide(state);
            pipeline.reset();
            wait(state, 100ms);
            continue;
        }
        if (state.stop || state.paused) continue;
        // HWND motion and CSS shape updates can arrive while DXGI is waiting.
        // Sample at the latest position so the lens follows native dragging.
        surface = current_surface(state);
        if (!surface || MonitorFromRect(&surface->clip, MONITOR_DEFAULTTONULL) != monitor) continue;

        RECT sample_region = surface->bounds, overlap{};
        const auto margin = std::lround(kMargin * surface->scale);
        InflateRect(&sample_region, margin, margin);
        const bool dirty_hit = IntersectRect(&overlap, &dirty, &sample_region);
        const bool moved = !pipeline->last_render || !pipeline->last_render->matches(*surface);
        if (const auto luma = pipeline->renderer.read_luma(); luma && !moved) state.luma = *luma;
        if ((moved || dirty_hit) && pipeline->capture.view()) {
            if (!pipeline->renderer.render(pipeline->capture.view(), *surface, pipeline->companion)) {
                pipeline->hide(state);
                pipeline.reset();
                wait(state, 100ms);
                continue;
            }
            pipeline->last_render = surface;
        } else if (!moved) {
            pipeline->renderer.sample_static_frame();
        }

        if (pipeline->last_render && pipeline->last_render->matches(*surface)) {
            if (!pipeline->companion.place(anchor, surface->bounds)) {
                pipeline->hide(state);
                pipeline.reset();
                wait(state, 100ms);
                continue;
            }
            state.ready = true;
        } else if (moved) {
            pipeline->hide(state);
        }
    }
}

void start_worker(const GlassHandle& state) noexcept {
    SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    const HRESULT apartment = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    try {
        run(*state);
    } catch (...) {
        // Surface creation and capture failures return control to the WebView.
    }
    if (SUCCEEDED(apartment)) CoUninitialize();
    state->ready = false;
    state->luma = -1;
}

}  // namespace

void* agenthud_glass_start(std::intptr_t raw_anchor) noexcept {
    const auto anchor = reinterpret_cast<HWND>(raw_anchor);
    DWORD process = 0;
    if (!GetWindowThreadProcessId(anchor, &process) || process != GetCurrentProcessId() || !anchor_excluded(anchor)) {
        return nullptr;
    }
    try {
        const auto state = std::make_shared<WorkerState>(anchor);
        auto handle = std::make_unique<GlassHandle>(state);
        std::thread worker([state] { start_worker(state); });
        worker.detach();
        return handle.release();
    } catch (...) {
        return nullptr;
    }
}

void agenthud_glass_stop(void* handle) noexcept {
    std::unique_ptr<GlassHandle> owner(static_cast<GlassHandle*>(handle));
    (*owner)->stop = true;
    (*owner)->wake.notify_one();
}

void agenthud_glass_set_surface(void* handle, const AgentHudGlassSurface* surface) noexcept {
    auto& state = **static_cast<GlassHandle*>(handle);
    {
        const std::lock_guard lock(state.mutex);
        state.surface = surface && valid(*surface) ? std::make_optional(*surface) : std::nullopt;
    }
    state.wake.notify_one();
}

void agenthud_glass_set_paused(void* handle, int paused) noexcept {
    auto& state = **static_cast<GlassHandle*>(handle);
    if (state.paused.exchange(paused != 0) != (paused != 0)) state.wake.notify_one();
}

int agenthud_glass_is_ready(const void* handle) noexcept {
    return (**static_cast<const GlassHandle*>(handle)).ready ? 1 : 0;
}

int agenthud_glass_luma(const void* handle) noexcept {
    return (**static_cast<const GlassHandle*>(handle)).luma;
}
