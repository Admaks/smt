//! 登录模块
//! 
//! 该模块负责处理用户登录功能，包括二维码登录的生成、检查和页面绑定。

use image::Rgba;
use slint::ComponentHandle;
use smt::app_lib::AppLib;
use ncm_api_rust::model::QrCode;
use crate::{*};


async fn load_qrcode(app_weak: slint::Weak<AppWindow>, app_lib: AppLibRc) -> QrCode {
    // 获取应用实例
    let app = app_weak.upgrade().expect("Cannot get app object");
    // 设置登录状态为未登录
    app.global::<AppStatus>().set_logined(false);

    let qr_code = app_lib
        .client
        .login_qrcode()
        .await
        .unwrap();

    // 定义二维码的前景和背景颜色
    let foreground = Rgba([0, 120, 212, 255]);
    let background = Rgba([0, 0, 0, 0]);
    // 生成二维码
    let qr_code_path = app_lib.config.cache_dir.join("qrcode.png");
    AppLib::generate_qrcode_image(
        &qr_code.url,
        400,
        &qr_code_path,
        foreground,
        background);

    // 将二维码图像加载到UI中
    app.global::<LoginProperty>().set_qr_img(
        slint::Image::load_from_path(&qr_code_path).expect("Cannot load qrcode image")
    );

    qr_code
}

/// 绑定二维码登录功能
/// 
/// 该函数生成二维码图像，设置到UI中，并绑定检查二维码的回调。
async fn bind_qrcode_login(app_weak: slint::Weak<AppWindow>, app_lib: AppLibRc) {
    let qr_code = load_qrcode(app_weak.clone(), app_lib.clone()).await;
    let app = app_weak.upgrade().unwrap();

    // 绑定检查二维码的回调函数
    app.global::<LoginProperty>().on_check_qrcode(move || {
        let qr_code = qr_code.clone();
        let app_weak = app_weak.clone();
        let app_lib = app_lib.clone();
        let _ = slint::spawn_local(async move {
            if let Ok(cookie_str) = app_lib.client.login_check_qrcode(qr_code).await
            {
                println!("Login success, cookie:\n {}", cookie_str);
                app_lib.init(&cookie_str).await.unwrap();
                let app = app_weak.upgrade().unwrap();
                app.global::<AppStatus>().set_logined(true);
                app.global::<LoginProperty>().invoke_login_ready();
                return;
            } else {
                let app = app_weak.upgrade().unwrap();
                app.global::<LoginProperty>().set_check_qrcode_failed(true);
                app.global::<LoginProperty>().set_waiting_response(false);
                bind_qrcode_login(app_weak, app_lib).await;
            }
        });
    });
}

/// 绑定登录页面
/// 
/// 该函数初始化登录页面，启动二维码登录绑定，并设置重新加载二维码的回调。
pub fn bind_login_page(app_weak: slint::Weak<AppWindow>, ncm_api: AppLibRc) {
    // 获取应用实例
    // let app = app_weak.upgrade().unwrap();
    // 异步启动二维码登录绑定
    let _ = slint::spawn_local(async move {
        bind_qrcode_login(app_weak, ncm_api).await;
    });
}
