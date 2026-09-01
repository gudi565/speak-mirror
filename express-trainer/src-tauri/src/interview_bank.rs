//! 模拟面试离线题库（interview.rs 的降级数据源）。
//!
//! 纯 const 数据 + 两个纯函数（题库查找 / 随机抽取），便于单测。
//! 未配置 AI / 网络失败 / 两次解析失败时由 interview.rs 走到这里，
//! 保证模拟面试在无 Key 机器上可用。

/// 一道内置面试题：题面 + 考察点
pub struct BankQuestion {
    pub question: &'static str,
    pub intent: &'static str,
}

/// 通用面试（12 道）
pub const GENERAL: &[BankQuestion] = &[
    BankQuestion {
        question: "请用一分钟做个自我介绍。",
        intent: "开场定位与信息密度：能否短时间内讲清我是谁、最强的一点",
    },
    BankQuestion {
        question: "说说你最大的优点和缺点各一个。",
        intent: "自我认知与分寸感：优缺点能否给出具体证据",
    },
    BankQuestion {
        question: "为什么想加入我们公司？",
        intent: "动机表达的结构：结论先行 + 对公司的了解 + 个人匹配点",
    },
    BankQuestion {
        question: "你未来三年的职业规划是什么？",
        intent: "目标表达的层次感与逻辑递进",
    },
    BankQuestion {
        question: "讲一次你在压力下完成任务的经历。",
        intent: "STAR 完整性与抗压叙述的条理性",
    },
    BankQuestion {
        question: "你和同事发生分歧时怎么处理？",
        intent: "冲突叙事的行为细节与结果意识",
    },
    BankQuestion {
        question: "讲一件你失败的事，以及你从中学到了什么。",
        intent: "复盘表达：能否坦诚讲清经过并带出改变",
    },
    BankQuestion {
        question: "你为什么离开上一家公司？",
        intent: "敏感问题的分寸与结论先行的直接性",
    },
    BankQuestion {
        question: "你最有成就感的一件事是什么？",
        intent: "成果叙述的量化意识与个人贡献边界",
    },
    BankQuestion {
        question: "如果入职后发现工作内容和预期不符，你会怎么办？",
        intent: "假设性问题的应对结构：先立场后展开",
    },
    BankQuestion {
        question: "你平时怎么持续学习？举个例子。",
        intent: "举例能力：观点之后能否立刻给出具体事实",
    },
    BankQuestion {
        question: "给我三个录用你的理由。",
        intent: "归纳与排序能力：理由是否互斥且各有支撑",
    },
];

/// 后端开发（9 道）
pub const BACKEND: &[BankQuestion] = &[
    BankQuestion {
        question: "介绍一个你负责过的系统架构，以及你做出的关键取舍。",
        intent: "架构表达：能否结论先行讲清取舍理由",
    },
    BankQuestion {
        question: "线上服务突然变慢，说说你的排查思路。",
        intent: "排查叙事的条理：现象、假设、验证、结论",
    },
    BankQuestion {
        question: "讲一次你处理过的最棘手的线上故障。",
        intent: "STAR 完整性与复盘价值",
    },
    BankQuestion {
        question: "数据库慢查询你怎么定位和优化？",
        intent: "技术方案表达的步骤感与量化收益",
    },
    BankQuestion {
        question: "讲讲你对高并发场景的理解，举你做过的例子。",
        intent: "抽象概念与具体案例的结合能力",
    },
    BankQuestion {
        question: "你怎么保障代码质量？",
        intent: "流程叙述的完整性与个人角色",
    },
    BankQuestion {
        question: "服务上线前你会做哪些准备？",
        intent: "清单式表达的条理与优先级",
    },
    BankQuestion {
        question: "讲一次你和前端或产品因为接口设计产生的分歧。",
        intent: "跨职能沟通叙事与结果",
    },
    BankQuestion {
        question: "如果让你从零搭建一个新服务的监控体系，你会怎么做？",
        intent: "方案表达的结构：目标、分层、落地",
    },
];

/// 前端开发（9 道）
pub const FRONTEND: &[BankQuestion] = &[
    BankQuestion {
        question: "介绍一个你最得意的前端项目，你解决了什么核心问题？",
        intent: "项目叙事的 STAR 与个人贡献边界",
    },
    BankQuestion {
        question: "页面首屏加载慢，你的优化思路是什么？",
        intent: "性能优化表达的层次：度量、定位、手段、收益",
    },
    BankQuestion {
        question: "讲一次你处理过的复杂组件状态管理问题。",
        intent: "技术细节的化繁为简能力",
    },
    BankQuestion {
        question: "你怎么保证不同浏览器和设备上的体验一致？",
        intent: "方案叙述的完整性：规范、工具链、测试",
    },
    BankQuestion {
        question: "讲讲你对前端性能指标的理解，怎么在实际项目里改善它们？",
        intent: "指标量化意识与落地案例",
    },
    BankQuestion {
        question: "和设计师对不上稿时你怎么沟通？",
        intent: "协作叙事与换位表达",
    },
    BankQuestion {
        question: "讲一次你推动引入新技术的决策过程。",
        intent: "决策叙事：动机、对比、风险、结果",
    },
    BankQuestion {
        question: "线上一个偶现的交互 bug，你怎么定位？",
        intent: "排查叙事的方法论与韧性",
    },
    BankQuestion {
        question: "你怎么看组件化对前端团队的价值？",
        intent: "观点表达：立场先行 + 论据支撑",
    },
];

/// 产品经理（9 道）
pub const PRODUCT: &[BankQuestion] = &[
    BankQuestion {
        question: "讲一个你从零做到上线的功能，说说你的完整决策过程。",
        intent: "产品叙事：目标、假设、验证、迭代",
    },
    BankQuestion {
        question: "你怎么判断一个需求该不该做？",
        intent: "决策框架表达：结论先行 + 判断依据分层",
    },
    BankQuestion {
        question: "数据指标涨了但你怀疑是坏变化，你会怎么办？",
        intent: "数据质疑的叙事与验证方法",
    },
    BankQuestion {
        question: "讲一次你和开发在排期上谈不拢的经历。",
        intent: "冲突协调叙事与结果",
    },
    BankQuestion {
        question: "你怎么做竞品分析？举个例子。",
        intent: "方法论与案例的结合能力",
    },
    BankQuestion {
        question: "一个功能上线后数据不达预期，你怎么复盘？",
        intent: "复盘结构：目标回放、偏差定位、归因、行动",
    },
    BankQuestion {
        question: "你怎么平衡商业目标与用户体验？",
        intent: "权衡类问题的立场表达",
    },
    BankQuestion {
        question: "讲一个你说服别人接受你方案的经历。",
        intent: "说服叙事：听众视角与证据组织",
    },
    BankQuestion {
        question: "如果让你负责一个完全陌生领域的产品，头一个月你做什么？",
        intent: "计划表达的条理与优先级",
    },
];

/// 运营（9 道）
pub const OPS: &[BankQuestion] = &[
    BankQuestion {
        question: "讲一次你策划的完整活动，从目标到复盘。",
        intent: "活动叙事：目标、玩法、执行、数据、复盘",
    },
    BankQuestion {
        question: "预算砍半但目标不变，你怎么调整策略？",
        intent: "权衡表达：优先级排序与取舍依据",
    },
    BankQuestion {
        question: "你怎么从零搭建一个新渠道的增长？",
        intent: "增长方法论的层次：假设、测试、放大",
    },
    BankQuestion {
        question: "讲一次活动数据不及预期的经历，你怎么应对？",
        intent: "应变叙事与复盘深度",
    },
    BankQuestion {
        question: "你怎么定义和拆解运营的核心指标？",
        intent: "指标表达的逻辑树与层次",
    },
    BankQuestion {
        question: "讲一次你跨部门协调资源的经历。",
        intent: "协作叙事与说服过程",
    },
    BankQuestion {
        question: "内容运营怎么做差异化？举个例子。",
        intent: "观点与案例的结合能力",
    },
    BankQuestion {
        question: "你怎么处理一次突发的舆情或客诉？",
        intent: "危机叙事：响应顺序与分寸",
    },
    BankQuestion {
        question: "你怎么看私域运营的价值？",
        intent: "观点表达：立场、论据、边界",
    },
];

/// 管理岗位（9 道）
pub const MANAGEMENT: &[BankQuestion] = &[
    BankQuestion {
        question: "你怎么带一个新组建的团队？",
        intent: "管理叙事：诊断、节奏、机制",
    },
    BankQuestion {
        question: "讲一次你做过的最难的绩效沟通。",
        intent: "敏感沟通叙事的分寸与结构",
    },
    BankQuestion {
        question: "团队士气低落时你做过什么？",
        intent: "情境应对的 STAR 与共情表达",
    },
    BankQuestion {
        question: "你怎么给团队定目标并保证落地？",
        intent: "目标管理叙事：拆解、对齐、追踪",
    },
    BankQuestion {
        question: "讲一次你淘汰或调整团队成员的经历。",
        intent: "决策叙事的坦诚与流程感",
    },
    BankQuestion {
        question: "向上管理和向下管理有什么不同？举个例子。",
        intent: "抽象对比与案例能力",
    },
    BankQuestion {
        question: "跨部门项目推不动，你怎么破局？",
        intent: "影响力叙事：利益分析、策略、结果",
    },
    BankQuestion {
        question: "你怎么培养接班人或团队骨干？",
        intent: "培养叙事的层次与具体动作",
    },
    BankQuestion {
        question: "讲一个你做过的最艰难的业务决策。",
        intent: "决策叙事：信息不全时的取舍与担当",
    },
];

/// 按岗位代号取题库；custom 无离线题库（出题必须靠 LLM + JD），离线降级走通用库
pub fn bank_for(role: &str) -> Option<&'static [BankQuestion]> {
    match role {
        "general" => Some(GENERAL),
        "backend" => Some(BACKEND),
        "frontend" => Some(FRONTEND),
        "product" => Some(PRODUCT),
        "ops" => Some(OPS),
        "management" => Some(MANAGEMENT),
        _ => None,
    }
}

/// xorshift64* 伪随机数（无需额外依赖；种子决定序列，可测）
fn xorshift64(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

/// 从题库随机抽 count 道（同一 seed 结果确定；不重复；count 超库容时返回全库）。
/// 返回值带 1 起始的 index，顺序为抽中顺序。
pub fn pick_offline(bank: &[BankQuestion], count: usize, seed: u64) -> Vec<(u32, &'static str, &'static str)> {
    let take = count.min(bank.len());
    // 部分 Fisher–Yates：前 take 位随机化
    let mut idx: Vec<usize> = (0..bank.len()).collect();
    let mut rng = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed }; // 全 0 种子换常数避免退化
    for i in 0..take {
        rng = xorshift64(rng);
        let j = i + (rng as usize) % (bank.len() - i);
        idx.swap(i, j);
    }
    idx[..take]
        .iter()
        .enumerate()
        .map(|(n, &i)| (n as u32 + 1, bank[i].question, bank[i].intent))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banks_have_expected_sizes_and_nonempty_fields() {
        assert_eq!(GENERAL.len(), 12);
        for (name, bank) in [
            ("backend", BACKEND),
            ("frontend", FRONTEND),
            ("product", PRODUCT),
            ("ops", OPS),
            ("management", MANAGEMENT),
        ] {
            assert!(
                (8..=10).contains(&bank.len()),
                "{name} 库容应在 8–10，实际 {}",
                bank.len()
            );
        }
        for (name, bank) in [
            ("general", GENERAL),
            ("backend", BACKEND),
            ("frontend", FRONTEND),
            ("product", PRODUCT),
            ("ops", OPS),
            ("management", MANAGEMENT),
        ] {
            for q in bank {
                assert!(!q.question.trim().is_empty(), "{name} 有空题面");
                assert!(!q.intent.trim().is_empty(), "{name} 有空考察点");
                assert!(q.question.chars().count() <= 60, "{name} 题面超长：{}", q.question);
            }
        }
    }

    #[test]
    fn bank_for_maps_all_roles_and_custom_is_none() {
        for role in ["general", "backend", "frontend", "product", "ops", "management"] {
            assert!(bank_for(role).is_some(), "{role} 应有题库");
        }
        assert!(bank_for("custom").is_none());
        assert!(bank_for("bogus").is_none());
    }

    #[test]
    fn pick_offline_is_unique_and_deterministic_per_seed() {
        let a = pick_offline(GENERAL, 5, 42);
        let b = pick_offline(GENERAL, 5, 42);
        assert_eq!(a, b, "同种子应完全一致");
        assert_eq!(a.len(), 5);
        // 题面不重复
        let mut seen = std::collections::HashSet::new();
        for (_, q, _) in &a {
            assert!(seen.insert(*q), "出现重复题：{q}");
        }
        // index 从 1 连续
        for (n, (i, _, _)) in a.iter().enumerate() {
            assert_eq!(*i, n as u32 + 1);
        }
        // 不同种子大概率给出不同组合（固定两枚种子的确定性断言）
        let c = pick_offline(GENERAL, 5, 43);
        assert_ne!(a, c);
    }

    #[test]
    fn pick_offline_caps_at_bank_size_and_handles_zero() {
        assert_eq!(pick_offline(BACKEND, 99, 7).len(), BACKEND.len());
        assert!(pick_offline(GENERAL, 0, 7).is_empty());
    }
}
