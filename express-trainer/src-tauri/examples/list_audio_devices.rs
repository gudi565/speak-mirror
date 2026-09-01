// 诊断：列出 cpal 视角下所有宿主与输入/输出设备
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    println!("默认宿主: {:?}", cpal::default_host().id());
    for host_id in cpal::available_hosts() {
        let host = match cpal::host_from_id(host_id) {
            Ok(h) => h,
            Err(e) => {
                println!("宿主 {:?} 初始化失败: {}", host_id, e);
                continue;
            }
        };
        println!("\n== 宿主 {:?} ==", host_id);
        match host.input_devices() {
            Ok(devs) => {
                let list: Vec<String> = devs
                    .map(|d| match d.name() {
                        Ok(n) => n,
                        Err(e) => format!("<取名称失败: {}>", e),
                    })
                    .collect();
                if list.is_empty() {
                    println!("  输入设备: （无）");
                } else {
                    println!("  输入设备: {:?}", list);
                }
            }
            Err(e) => println!("  枚举输入设备失败: {}", e),
        }
        println!(
            "  默认输入: {:?}",
            host.default_input_device().and_then(|d| d.name().ok())
        );
        println!(
            "  默认输出: {:?}",
            host.default_output_device().and_then(|d| d.name().ok())
        );
    }
}
