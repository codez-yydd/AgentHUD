//! macOS 灵动岛窗口的「不钳制 frame」修复。
//!
//! # 背景
//!
//! 灵动岛是 780×632 的逻辑画布，主体（胶囊）在画布内四周留 `BODY_INSET=70`。
//! 贴顶吸附时窗口顶 y = work_area.y - 70 ≈ -40 逻辑点，即窗口越出屏幕顶。
//! Windows 允许窗口越界，一切正常；macOS 上 tao 的 `set_outer_position` 走到
//! `setFrameTopLeftPoint:`，NSWindow 的 `constrainFrameRect:toScreen:` 默认实现
//! 会把越界窗口钳回 visibleFrame 内（顶被推到菜单栏下方 +30 点），主体因此
//! 整体下移约 70 点。`setFrameOrigin:` 同样被钳制，换 API 无效，tao 0.35.3
//! 也没有关闭钳制的开关。
//!
//! # 做法：替换方法实现（不 isa-swizzle）
//!
//! 只对**灵动岛窗口实例**让 `constrainFrameRect:toScreen:` 直接返回原 frame。
//! 实现方式是在窗口实例当前的 class 上用 `class_replaceMethod` 安装一个覆写，
//! 覆写内部用实例指针判断：**只有岛窗口返回原 frame，其它实例转发给原实现**，
//! 从而不影响其它窗口的默认钳制行为。
//!
//! ## 为什么不做 isa-swizzle（重要）
//!
//! tao 创建的窗口不是纯 NSWindow，而是它注册的 `TaoWindow` 子类；且 WebView
//! 的 KVO 让它实际是 `NSKVONotifying_TaoWindow`。tao 的 `sendEvent:` 实现里用
//! `util::superclass(this)`（即 `[this superclass]`）计算 super 类再调
//! `[super sendEvent:]`。若用 object_setClass 把实例换成「以当前类为父类」的新
//! 子类，继承链多一层，`[this superclass]` 随之偏移，`[super sendEvent:]` 会
//! 递归调回 tao 自己的实现，导致 SIGSEGV（实测崩溃栈：tao::send_event →
//! util::superclass → -[NSObject superclass]）。因此**不能改变实例的 isa**，
//! 只能在类的方法表上替换 IMP，保持 `[this superclass]` 不变。
//!
//! # 约束
//!
//! - `install` 必须在主线程调用（NSWindow 访问要求主线程）；
//! - 方法替换与类缓存只做一次（OnceLock）；实例指针用 AtomicUsize 记录，可幂等
//!   重复安装；窗口隐藏再显示无需重装，禁用再启用会重建窗口，故每次建窗后都调用；
//! - 已知限制：tao 的 window_position 用 CGDisplay::main()（主屏高度）做 y
//!   基准，岛在外接屏时的换算属 tao 上游问题，本修复以主屏/单屏为准。

use objc2::ffi;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::ClassType;
use objc2_app_kit::NSWindow;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// 当前灵动岛窗口实例的指针（裸地址），用于在新 IMP 里做实例级 gate。
static ISLAND_WINDOW_PTR: AtomicUsize = AtomicUsize::new(0);

/// 原 `constrainFrameRect:toScreen:` 实现（NSWindow 的），供非岛实例转发。
/// 存成函数指针：`fn(self, _cmd, frame, screen) -> frame`。
type ConstrainImp = unsafe extern "C-unwind" fn(&AnyObject, Sel, objc2_foundation::NSRect, *const c_void) -> objc2_foundation::NSRect;
static ORIGINAL_IMP: OnceLock<ConstrainImp> = OnceLock::new();
/// 是否已在目标类上完成方法替换。
static INSTALLED: OnceLock<()> = OnceLock::new();

/// 新的 `constrainFrameRect:toScreen:` 实现。
///
/// - 岛窗口：原样返回 `frame`（不钳制）；
/// - 其它窗口：调用原 NSWindow 实现，保持默认钳制行为不变。
unsafe extern "C-unwind" fn constrain_frame_rect(
    this: &AnyObject,
    sel: Sel,
    frame: objc2_foundation::NSRect,
    screen: *const c_void,
) -> objc2_foundation::NSRect {
    let this_ptr = this as *const AnyObject as usize;
    if this_ptr == ISLAND_WINDOW_PTR.load(Ordering::Relaxed) {
        return frame;
    }
    if let Some(orig) = ORIGINAL_IMP.get() {
        return orig(this, sel, frame, screen);
    }
    frame
}

/// 让指定 NSWindow 实例的 `constrainFrameRect:toScreen:` 不再钳制。
///
/// `window` 是 `WebviewWindow::ns_window()` 返回的裸指针。必须在主线程调用。
pub(crate) fn install_no_clamp(window: *mut c_void) {
    ISLAND_WINDOW_PTR.store(window as usize, Ordering::Relaxed);

    INSTALLED.get_or_init(|| unsafe {
        let obj: &AnyObject = &*window.cast::<AnyObject>();
        // 实例当前的 class（TaoWindow / NSKVONotifying_TaoWindow）。
        let cls: &AnyClass = obj.class();
        let sel = objc2::sel!(constrainFrameRect:toScreen:);
        // 沿继承链找到原实现（通常是 NSWindow 的），拿到它的原始 IMP 与类型编码。
        let Some(method) = cls.instance_method(sel) else {
            eprintln!("[zbar-island] 未找到 constrainFrameRect:toScreen:，跳过防钳制安装");
            return;
        };
        let orig_imp = method.implementation();
        let enc: *const c_char = ffi::method_getTypeEncoding(method as *const _ as *const objc2::runtime::Method);
        if enc.is_null() {
            eprintln!("[zbar-island] 无法读取 constrainFrameRect 类型编码，跳过安装");
            return;
        }
        let orig_fn: ConstrainImp = std::mem::transmute(orig_imp);
        let _ = ORIGINAL_IMP.set(orig_fn);

        // 在当前 class 上替换为新 IMP。class_replaceMethod 会保留父类原实现
        // （我们已保存其 IMP 用于转发），只在此 class 上新增/覆盖入口。
        let new_imp: Imp = std::mem::transmute(
            constrain_frame_rect
                as unsafe extern "C-unwind" fn(&AnyObject, Sel, objc2_foundation::NSRect, *const c_void) -> objc2_foundation::NSRect,
        );
        let replaced = ffi::class_replaceMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel,
            new_imp,
            enc,
        );
        if replaced.is_some() {
            eprintln!("[zbar-island] constrainFrameRect:toScreen: 已被替换过（保留最新实例指针）");
        }
    });

    // NSWindow 声明该方法需要 NSScreen feature；仅用于确保类型可见。
    let _ = NSWindow::class();
}
