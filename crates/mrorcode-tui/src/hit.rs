//! 鼠标命中区域模型（M4-5）。
//!
//! 渲染时把可点击区域的屏幕矩形记入 `AppState.hit_regions`（每帧重建），
//! 鼠标事件据此分派；命中测试与渲染解耦，可用单测覆盖，不依赖真实终端。

use ratatui::layout::Rect;

/// 可点击目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitTarget {
    /// 切换到某个会话标签/侧栏条目。
    Session(u64),
    /// 关闭某个会话标签。
    CloseSession(u64),
    /// 聚焦（并切换展开）某个工具卡。
    Tool(String),
    /// 展开/收起某条消息的思考过程。
    Reasoning(u64),
    /// 打开某工具卡的 diff viewer。
    Diff(String),
    /// 折叠/展开侧栏分区标题（M4-8）。
    SidebarSection(String),
}

/// 一个命中区域。
#[derive(Clone, Debug)]
pub struct HitRegion {
    pub rect: Rect,
    pub target: HitTarget,
}

impl HitRegion {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.rect.x
            && x < self.rect.x + self.rect.width
            && y >= self.rect.y
            && y < self.rect.y + self.rect.height
    }
}

/// 命中测试：返回最上层（后加入者优先）匹配的目标。
pub fn hit_test(regions: &[HitRegion], x: u16, y: u16) -> Option<HitTarget> {
    regions
        .iter()
        .rev()
        .find(|r| r.contains(x, y))
        .map(|r| r.target.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u16, y: u16, w: u16, h: u16, t: HitTarget) -> HitRegion {
        HitRegion {
            rect: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            target: t,
        }
    }

    #[test]
    fn hit_test_finds_overlapping_topmost() {
        let regions = vec![
            r(0, 0, 10, 1, HitTarget::Session(1)),
            r(0, 0, 5, 1, HitTarget::Tool("t1".into())),
        ];
        assert_eq!(hit_test(&regions, 2, 0), Some(HitTarget::Tool("t1".into())));
        assert_eq!(hit_test(&regions, 7, 0), Some(HitTarget::Session(1)));
        assert_eq!(hit_test(&regions, 0, 5), None);
    }

    #[test]
    fn contains_respects_half_open_bounds() {
        let region = r(3, 2, 4, 2, HitTarget::Session(9));
        assert!(region.contains(3, 2));
        assert!(region.contains(6, 3));
        assert!(!region.contains(7, 3));
        assert!(!region.contains(3, 4));
    }
}
