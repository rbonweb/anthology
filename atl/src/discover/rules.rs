use super::report::AtlStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineSafety {
    None,
    ProducerOnly,
    #[allow(dead_code)]
    FinalOnly,
    Both,
}

impl PipelineSafety {
    pub fn producer_safe(self) -> bool {
        matches!(self, Self::ProducerOnly | Self::Both)
    }

    pub fn final_safe(self) -> bool {
        matches!(self, Self::FinalOnly | Self::Both)
    }
}

pub struct AtlRule {
    pub pattern: &'static str,
    pub atl_cmd: &'static str,
    /// Pipeline stage positions this command may be rewritten in (#3171).
    pub pipeline_safety: PipelineSafety,
    pub rewrite_prefixes: &'static [&'static str],
    pub category: &'static str,
    pub savings_pct: f64,
    pub subcmd_savings: &'static [(&'static str, f64)],
    pub subcmd_status: &'static [(&'static str, AtlStatus)],
}

impl AtlRule {
    // `Default::default()` isn't a const fn on stable, so `RULES` (a const array) can't call
    // it via `..Default::default()` — hence this associated const as the const-context
    // workaround. Once `const_trait_impl` stabilizes, drop this and derive/impl
    // `const Default` instead, then switch call sites back to `..AtlRule::default()`.
    pub const DEFAULT: AtlRule = AtlRule {
        pattern: "",
        atl_cmd: "",
        pipeline_safety: PipelineSafety::None,
        rewrite_prefixes: &[],
        category: "",
        savings_pct: 60.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    };
}

impl Default for AtlRule {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A rule declaring subcommands also needs its tool in
/// `core::tracking::SUBCOMMAND_ROUTERS`, or its telemetry label stops at the tool name.
pub const RULES: &[AtlRule] = &[
    AtlRule {
        pattern: r"^(?:git|yadm)\s+(?:-[Cc]\s+\S+\s+)*(status|log|diff|show|add|commit|checkout|push|pull|branch|fetch|stash|worktree|ls-tree)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl git",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["git", "yadm"],
        category: "Git",
        savings_pct: 70.0,
        subcmd_savings: &[
            ("diff", 80.0),
            ("show", 80.0),
            ("add", 59.0),
            ("commit", 59.0),
            ("ls-tree", 37.0),
        ],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^gh\s+(pr|issue|run|repo|api|release)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl gh",
        rewrite_prefixes: &["gh"],
        category: "GitHub",
        savings_pct: 82.0,
        subcmd_savings: &[("pr", 87.0), ("run", 82.0), ("issue", 80.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^glab\s+(mr|issue|ci|pipeline|api|release)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl glab",
        rewrite_prefixes: &["glab"],
        category: "GitLab",
        savings_pct: 82.0,
        subcmd_savings: &[("mr", 87.0), ("ci", 82.0), ("issue", 80.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^cargo\s+(build|test|clippy|check|fmt|install)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl cargo",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["cargo"],
        category: "Cargo",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("check", 80.0)],
        subcmd_status: &[("fmt", AtlStatus::Passthrough)],
    },
    AtlRule {
        // `lint` is the package script of that name, not a linter: `pnpm lint` runs the
        // project's own `scripts.lint` (biome, oxlint, eslint with flags, anything), so it
        // routes to the script runner like `pnpm run lint` and never to `atl lint`.
        pattern: r"^pnpm\s+(exec|i|install|list|ls|outdated|run|run-script|lint(?:\s|$))",
        atl_cmd: "atl pnpm",
        rewrite_prefixes: &["pnpm"],
        category: "PackageManager",
        savings_pct: 80.0,
        // `atl pnpm exec|run|run-script|lint` hand the command to pnpm unchanged and unfiltered.
        subcmd_status: &[
            ("exec", AtlStatus::Passthrough),
            ("run", AtlStatus::Passthrough),
            ("run-script", AtlStatus::Passthrough),
            ("lint", AtlStatus::Passthrough),
        ],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^npm\s+(exec|run|run-script|rum|urn|x)(\s|$)",
        atl_cmd: "atl npm",
        rewrite_prefixes: &["npm"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^npx\s+",
        atl_cmd: "atl npx",
        rewrite_prefixes: &["npx"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(cat|head|tail)\s+",
        atl_cmd: "atl read",
        rewrite_prefixes: &["cat", "head", "tail"],
        category: "Files",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^grep\s+",
        atl_cmd: "atl grep",
        pipeline_safety: PipelineSafety::Both,
        rewrite_prefixes: &["grep"],
        category: "Files",
        savings_pct: 75.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^rg\s+",
        atl_cmd: "atl rg",
        pipeline_safety: PipelineSafety::Both,
        rewrite_prefixes: &["rg"],
        category: "Files",
        savings_pct: 75.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ast-grep\s+",
        atl_cmd: "atl ast-grep",
        // Unlike grep/rg, `atl ast-grep`'s run() captures with stdin null on the
        // path it filters, so it must not be rewritten as a pipeline's final
        // stage — that would silently drop the pipe input. `run --stdin` and the
        // other subcommands do reach the child's stdin, but only when invoked
        // directly: this rule keeps the hook from producing that form at all.
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ast-grep"],
        category: "Files",
        savings_pct: 85.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ls(\s|$)",
        atl_cmd: "atl ls",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ls"],
        category: "Files",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^find\s+",
        atl_cmd: "atl find",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["find"],
        category: "Files",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?tsc(\s|$)",
        atl_cmd: "atl tsc",
        rewrite_prefixes: &[
            "npm exec tsc",
            "npm tsc",
            "npm x tsc",
            "npx tsc",
            "pnpm dlx tsc",
            "pnpm exec tsc",
            "pnpm tsc",
            "pnpx tsc",
            "tsc",
        ],
        category: "Build",
        savings_pct: 83.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        // `atl lint biome` keeps the tool name: `atl lint` picks its linter from the first
        // argument, so dropping `biome` here would hand `check`, `.` or `src` to it as a tool.
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?biome(\s|$)",
        atl_cmd: "atl lint biome",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "biome",
            "npm biome",
            "npm exec biome",
            "npm x biome",
            "npx biome",
            "pnpm biome",
            "pnpm dlx biome",
            "pnpm exec biome",
            "pnpx biome",
        ],
        category: "Build",
        savings_pct: 84.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        // `atl lint eslint` keeps the tool name: `atl lint` picks its linter from the first
        // argument, so dropping `eslint` here would hand `check`, `.` or `src` to it as a tool.
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?eslint(\s|$)",
        atl_cmd: "atl lint eslint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "eslint",
            "npm eslint",
            "npm exec eslint",
            "npm x eslint",
            "npx eslint",
            "pnpm eslint",
            "pnpm dlx eslint",
            "pnpm exec eslint",
            "pnpx eslint",
        ],
        category: "Build",
        savings_pct: 84.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?prettier(?:\s|$|[;|&()<>])",
        atl_cmd: "atl prettier",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "npm exec prettier",
            "npm prettier",
            "npm x prettier",
            "npx prettier",
            "pnpm dlx prettier",
            "pnpm exec prettier",
            "pnpm prettier",
            "pnpx prettier",
            "prettier",
        ],
        category: "Build",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ng\s+build(?:\s|$|[;|&()<>])",
        atl_cmd: "atl ng",
        rewrite_prefixes: &["ng"],
        category: "Build",
        // Angular 22.2.1 real default/lazy build fixtures; diagnostics remain intact.
        savings_pct: 20.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?next\s+build(?:\s|$|[;|&()<>])",
        atl_cmd: "atl next",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "next build",
            "npm exec next build",
            "npm next build",
            "npm x next build",
            "npx next build",
            "pnpm dlx next build",
            "pnpm exec next build",
            "pnpm next build",
            "pnpx next build",
        ],
        category: "Build",
        savings_pct: 87.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?jest(\s+run)?(\s|$)",
        atl_cmd: "atl jest",
        rewrite_prefixes: &[
            "jest run",
            "jest",
            "npm exec jest run",
            "npm exec jest",
            "npm jest run",
            "npm jest",
            "npm x jest run",
            "npm x jest",
            "npx jest run",
            "npx jest",
            "pnpm dlx jest run",
            "pnpm dlx jest",
            "pnpm exec jest run",
            "pnpm exec jest",
            "pnpm jest run",
            "pnpm jest",
            "pnpx jest run",
            "pnpx jest",
        ],
        category: "Tests",
        savings_pct: 99.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?vitest(\s+run)?(\s|$)",
        atl_cmd: "atl vitest",
        rewrite_prefixes: &[
            "npm exec vitest run",
            "npm exec vitest",
            "npm vitest run",
            "npm vitest",
            "npm x vitest run",
            "npm x vitest",
            "npx vitest run",
            "npx vitest",
            "pnpm dlx vitest run",
            "pnpm dlx vitest",
            "pnpm exec vitest run",
            "pnpm exec vitest",
            "pnpm vitest run",
            "pnpm vitest",
            "pnpx vitest run",
            "pnpx vitest",
            "vitest run",
            "vitest",
        ],
        category: "Tests",
        savings_pct: 99.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ctest(?:\s|$)",
        atl_cmd: "atl ctest",
        rewrite_prefixes: &["ctest"],
        category: "Tests",
        savings_pct: 80.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?playwright(?:\s|$|[;|&()<>])",
        atl_cmd: "atl playwright",
        rewrite_prefixes: &[
            "npm exec playwright",
            "npm playwright",
            "npm x playwright",
            "npx playwright",
            "playwright",
            "pnpm dlx playwright",
            "pnpm exec playwright",
            "pnpm playwright",
            "pnpx playwright",
        ],
        category: "Tests",
        savings_pct: 94.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^((p?np(m|x)|p?npm\s+exec|npm\s+x|pnpm\s+dlx)\s+)?prisma(?:\s|$|[;|&()<>])",
        atl_cmd: "atl prisma",
        rewrite_prefixes: &[
            "npm exec prisma",
            "npm prisma",
            "npm x prisma",
            "npx prisma",
            "pnpm dlx prisma",
            "pnpm exec prisma",
            "pnpm prisma",
            "pnpx prisma",
            "prisma",
        ],
        category: "Build",
        savings_pct: 88.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^docker\s+(ps|images|logs|run|exec|build|compose\s+(ps|logs|build))(?:\s|$|[;|&()<>])",
        atl_cmd: "atl docker",
        rewrite_prefixes: &["docker"],
        category: "Infra",
        savings_pct: 85.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^kubectl\s+(get|logs|describe|apply)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl kubectl",
        rewrite_prefixes: &["kubectl"],
        category: "Infra",
        savings_pct: 85.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^oc\s+(get|logs|describe|apply|status|adm)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl oc",
        rewrite_prefixes: &["oc"],
        category: "Infra",
        savings_pct: 85.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^tree(\s|$)",
        atl_cmd: "atl tree",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["tree"],
        category: "Files",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^diff\s+",
        atl_cmd: "atl diff",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["diff"],
        category: "Files",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^curl\s+",
        atl_cmd: "atl curl",
        rewrite_prefixes: &["curl"],
        category: "Network",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^wget\s+",
        atl_cmd: "atl wget",
        rewrite_prefixes: &["wget"],
        category: "Network",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    // Generic python before mypy/pytest so specific rules (last-match-wins)
    // still override this one for `python -m mypy` / `python -m pytest`.
    AtlRule {
        pattern: r"^python3?(?:\s|$)",
        atl_cmd: "atl python",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["python", "python3"],
        category: "Python",
        savings_pct: 40.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(python3?\s+-m\s+)?mypy(\s|$)",
        atl_cmd: "atl mypy",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["python3 -m mypy", "python -m mypy", "mypy"],
        category: "Build",
        savings_pct: 80.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ruff\s+(check|format)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl ruff",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ruff"],
        category: "Python",
        savings_pct: 80.0,
        subcmd_savings: &[("check", 80.0), ("format", 75.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^sqlfluff\s+lint(?:\s|$|[;|&()<>])",
        atl_cmd: "atl sqlfluff",
        rewrite_prefixes: &["sqlfluff"],
        category: "Python",
        savings_pct: 75.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(python[0-9.]*\s+-m\s+)?pytest(\s|$)",
        atl_cmd: "atl pytest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["python3 -m pytest", "python -m pytest", "pytest"],
        category: "Python",
        savings_pct: 90.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(pip3?|uv\s+pip)\s+(list|outdated|install|show)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl pip",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pip3", "pip", "uv pip"],
        category: "Python",
        savings_pct: 75.0,
        subcmd_savings: &[("list", 75.0), ("outdated", 80.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^uv\s+run(?:\s|$)",
        atl_cmd: "atl uv",
        rewrite_prefixes: &["uv"],
        category: "Python",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^go\s+(test|build|vet)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl go",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["go"],
        category: "Go",
        savings_pct: 85.0,
        subcmd_savings: &[("test", 90.0), ("build", 80.0), ("vet", 75.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:golangci-lint|golangci)\s+(run)(?:\s|$)",
        atl_cmd: "atl golangci-lint run",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["golangci-lint run", "golangci run"],
        category: "Go",
        savings_pct: 85.0,
        ..AtlRule::DEFAULT
    },
    // Scala/SBT
    AtlRule {
        pattern: r#"^sbt\s+["']?(testOnly|testQuick|test|compile|run|clean|assembly|package)(?:[\s"']|$)"#,
        atl_cmd: "atl sbt",
        rewrite_prefixes: &["sbt"],
        category: "Build",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("testOnly", 90.0), ("compile", 75.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^bundle\s+(install|update)\b",
        atl_cmd: "atl bundle",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle"],
        category: "Ruby",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:bundle\s+exec\s+)?(?:bin/)?(?:rake|rails)\s+test(?:[\s:]|$|[;|&()<>])",
        atl_cmd: "atl rake",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &[
            "bundle exec rails",
            "bundle exec rake",
            "bin/rails",
            "rails",
            "rake",
        ],
        category: "Ruby",
        savings_pct: 85.0,
        subcmd_savings: &[("test", 90.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:bundle\s+exec\s+)?rspec(?:\s|$)",
        atl_cmd: "atl rspec",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle exec rspec", "bin/rspec", "rspec"],
        category: "Tests",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:bundle\s+exec\s+)?rubocop(?:\s|$)",
        atl_cmd: "atl rubocop",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bundle exec rubocop", "rubocop"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^opencode\s+run(?:\s|$)",
        atl_cmd: "atl opencode",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["opencode"],
        category: "Build",
        savings_pct: 80.0,
        ..AtlRule::DEFAULT
    },
    // PHP tooling
    AtlRule {
        pattern: r"^php\s+artisan(?:\s|$)",
        atl_cmd: "atl php",
        rewrite_prefixes: &["php"],
        category: "Build",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^php\s+-l(?:\s|$)",
        atl_cmd: "atl php",
        rewrite_prefixes: &["php"],
        category: "Build",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^php\s+run-tests\.php(?:\s|$)",
        atl_cmd: "atl phpt",
        rewrite_prefixes: &["php run-tests.php"],
        category: "Tests",
        savings_pct: 99.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:php\s+)?(?:\./)?(?:(?:vendor/)?bin/)?phpunit(?:\s|$)",
        atl_cmd: "atl phpunit",
        pipeline_safety: PipelineSafety::ProducerOnly,
        // rewrite_segment_inner normalizes the php wrapper, `./`, vendor/bin and
        // composer bin-dir before matching, so only the residual forms remain:
        // a plain `bin/` (not a Composer dir, so it survives normalization) and
        // the bare tool name.
        rewrite_prefixes: &["bin/phpunit", "phpunit"],
        category: "Tests",
        savings_pct: 75.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:php\s+)?(?:\./)?(?:(?:vendor/)?bin/)?phpstan\s+analy[sz]e\b",
        atl_cmd: "atl phpstan",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["bin/phpstan", "phpstan"],
        category: "Build",
        savings_pct: 65.0,
        subcmd_savings: &[("analyse", 65.0), ("analyze", 65.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?pest(?:\s|$)",
        atl_cmd: "atl pest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pest"],
        category: "Tests",
        savings_pct: 80.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?paratest(?:\s|$)",
        atl_cmd: "atl paratest",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["paratest"],
        category: "Tests",
        savings_pct: 80.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?ecs(?:\s|$)",
        atl_cmd: "atl ecs",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ecs"],
        category: "Build",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./)?(?:vendor/bin/)?pint(?:\s|$)",
        atl_cmd: "atl pint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pint"],
        category: "Build",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^aws\s+",
        atl_cmd: "atl aws",
        rewrite_prefixes: &["aws"],
        category: "Infra",
        savings_pct: 80.0,
        subcmd_savings: &[
            ("sts", 80.0),
            ("s3", 60.0),
            ("ec2", 85.0),
            ("ecs", 90.0),
            ("rds", 80.0),
            ("cloudformation", 90.0),
            ("logs", 88.0),
            ("lambda", 90.0),
            ("iam", 85.0),
            ("dynamodb", 70.0),
            ("s3api", 75.0),
            ("eks", 87.0),
            ("sqs", 78.0),
            ("secretsmanager", 75.0),
        ],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^psql(\s|$)",
        atl_cmd: "atl psql",
        rewrite_prefixes: &["psql"],
        category: "Infra",
        savings_pct: 75.0,
        ..AtlRule::DEFAULT
    },
    // Bun/Deno
    AtlRule {
        pattern: r"^bun\s+(install|add|remove|test|build|run|pm\s+ls|pm|x)\b",
        atl_cmd: "atl bun",
        rewrite_prefixes: &["bun"],
        category: "PackageManager",
        savings_pct: 75.0,
        subcmd_savings: &[("test", 90.0), ("install", 70.0), ("pm ls", 70.0)],
        // Audited against what each subcommand actually does. "pm ls" is
        // filtered and every other "bun pm" is passthrough, which is why the
        // pattern captures the two-word form separately. "build" writes its
        // bundle to stdout unless an output flag is present, so its common form
        // runs unfiltered and it cannot claim the headline number.
        subcmd_status: &[
            ("run", AtlStatus::Passthrough),
            ("pm", AtlStatus::Passthrough),
            ("build", AtlStatus::Passthrough),
        ],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^bunx\s+",
        atl_cmd: "atl bunx",
        rewrite_prefixes: &["bunx"],
        category: "PackageManager",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^deno\s+(test|lint|check|run|task|compile|install)\b",
        atl_cmd: "atl deno",
        rewrite_prefixes: &["deno"],
        category: "Build",
        savings_pct: 75.0,
        // Measured against real deno 2.9.6 output: the lint and check filters
        // remove ANSI, download and blank lines and keep every diagnostic, so
        // they save bytes rather than content.
        subcmd_savings: &[("test", 90.0), ("lint", 40.0), ("check", 50.0)],
        // Audited alongside the bun rule above: test, lint and check are
        // filtered, the rest run unchanged.
        subcmd_status: &[
            ("run", AtlStatus::Passthrough),
            ("task", AtlStatus::Passthrough),
            ("install", AtlStatus::Passthrough),
            ("compile", AtlStatus::Passthrough),
        ],
        ..AtlRule::DEFAULT
    },
    // TOML-filtered commands
    AtlRule {
        pattern: r"^ansible-playbook\b",
        atl_cmd: "atl ansible-playbook",
        rewrite_prefixes: &["ansible-playbook"],
        category: "Infra",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^brew\s+(install|upgrade)\b",
        atl_cmd: "atl brew",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["brew"],
        category: "PackageManager",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^composer\s+(install|update|require)\b",
        atl_cmd: "atl composer",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["composer"],
        category: "PackageManager",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^df(\s|$)",
        atl_cmd: "atl df",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["df"],
        category: "System",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^dotnet\s+(build|restore)\b",
        atl_cmd: "atl dotnet",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["dotnet"],
        category: "Build",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^du\b",
        atl_cmd: "atl du",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["du"],
        category: "System",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^fail2ban-client\b",
        atl_cmd: "atl fail2ban-client",
        rewrite_prefixes: &["fail2ban-client"],
        category: "Infra",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^gcloud\b",
        atl_cmd: "atl gcloud",
        rewrite_prefixes: &["gcloud"],
        category: "Infra",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./gradlew|gradlew\.bat|gradlew|gradle)(?:\s+(test|build|clean|assemble\w*|install\w*|check|lint\w*|dependencies))?(\s|$)",
        atl_cmd: "atl gradlew",
        rewrite_prefixes: &["./gradlew", "gradlew.bat", "gradlew", "gradle"],
        category: "Build",
        savings_pct: 75.0,
        subcmd_savings: &[("test", 90.0), ("build", 80.0), ("check", 80.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^hadolint\b",
        atl_cmd: "atl hadolint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["hadolint"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^helm\b",
        atl_cmd: "atl helm",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["helm"],
        category: "Infra",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^iptables\b",
        atl_cmd: "atl iptables",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["iptables"],
        category: "Infra",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^make\b",
        atl_cmd: "atl make",
        rewrite_prefixes: &["make"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^markdownlint\b",
        atl_cmd: "atl markdownlint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["markdownlint"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^mix\s+(compile|format)(\s|$)",
        atl_cmd: "atl mix",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["mix"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^(?:\./mvnw|mvnw\.cmd|mvnw|mvn)\b(?:\s+\S+)*?\s+(compile|test|integration-test|package|install|verify|deploy)\b",
        atl_cmd: "atl mvn",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["./mvnw", "mvnw.cmd", "mvnw", "mvn"],
        category: "Build",
        savings_pct: 82.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        // `mvnd` is a separate binary, not a `mvn` wrapper — it must keep its
        // own atl_cmd so the daemon is what actually runs. mvnd ships
        // `mvnd.cmd` on Windows; listed explicitly (longer prefix first) on
        // both the pattern and rewrite_prefixes, mirroring the mvn rule's
        // `mvnw.cmd` handling — `^mvnd\b` alone matches the `.` boundary in
        // `mvnd.cmd` but can't then reach `\s+(compile|...)`, so it silently
        // fails to classify the command at all.
        pattern: r"^(?:mvnd\.cmd|mvnd)\b(?:\s+\S+)*?\s+(compile|test|integration-test|package|install|verify|deploy)\b",
        atl_cmd: "atl mvnd",
        rewrite_prefixes: &["mvnd.cmd", "mvnd"],
        category: "Build",
        savings_pct: 82.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ping\b",
        atl_cmd: "atl ping",
        rewrite_prefixes: &["ping"],
        category: "Network",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^pio\s+run(?:\s|$|[;|&()<>])",
        atl_cmd: "atl pio",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pio"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^poetry\s+(install|lock|update)\b",
        atl_cmd: "atl poetry",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["poetry"],
        category: "Python",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^pre-commit\b",
        atl_cmd: "atl pre-commit",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["pre-commit"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^ps(\s|$)",
        atl_cmd: "atl ps",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["ps"],
        category: "System",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^pulumi\s+(preview|up|destroy|refresh|stack)(\s|$)",
        atl_cmd: "atl pulumi",
        rewrite_prefixes: &["pulumi"],
        category: "Infra",
        savings_pct: 45.0,
        subcmd_savings: &[
            ("up", 66.0),
            ("destroy", 72.0),
            ("refresh", 35.0),
            ("preview", 25.0),
            ("stack", 29.0),
        ],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^quarto\s+render(?:\s|$|[;|&()<>])",
        atl_cmd: "atl quarto",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["quarto"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^rsync\b",
        atl_cmd: "atl rsync",
        rewrite_prefixes: &["rsync"],
        category: "Network",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^shellcheck\b",
        atl_cmd: "atl shellcheck",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["shellcheck"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^shopify\s+theme\s+(push|pull)(?:\s|$|[;|&()<>])",
        atl_cmd: "atl shopify",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["shopify"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^sops\b",
        atl_cmd: "atl sops",
        rewrite_prefixes: &["sops"],
        category: "Infra",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^swift\s+(build|test)\b",
        atl_cmd: "atl swift",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["swift"],
        category: "Build",
        savings_pct: 65.0,
        subcmd_savings: &[("test", 90.0)],
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^systemctl\s+status\b",
        atl_cmd: "atl systemctl",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["systemctl"],
        category: "System",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^terraform\s+plan(?:\s|$|[;|&()<>])",
        atl_cmd: "atl terraform",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["terraform"],
        category: "Infra",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^tofu\s+(fmt|init|plan|validate)(\s|$)",
        atl_cmd: "atl tofu",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["tofu"],
        category: "Infra",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^trunk\s+build(?:\s|$|[;|&()<>])",
        atl_cmd: "atl trunk",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["trunk"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^uv\s+(sync|pip\s+install)\b",
        atl_cmd: "atl uv",
        rewrite_prefixes: &["uv"],
        category: "Python",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^yamllint\b",
        atl_cmd: "atl yamllint",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["yamllint"],
        category: "Build",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^wc(\s|$)",
        atl_cmd: "atl wc",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["wc"],
        category: "Files",
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^gt\s+",
        atl_cmd: "atl gt",
        rewrite_prefixes: &["gt"],
        category: "Git",
        savings_pct: 70.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^liquibase(?:\s|$)",
        atl_cmd: "atl liquibase",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["liquibase"],
        category: "Infra",
        savings_pct: 65.0,
        ..AtlRule::DEFAULT
    },
    AtlRule {
        pattern: r"^node(?:\s|$)",
        atl_cmd: "atl node",
        pipeline_safety: PipelineSafety::ProducerOnly,
        rewrite_prefixes: &["node"],
        category: "Tests",
        savings_pct: 40.0,
        ..AtlRule::DEFAULT
    },
];

pub const IGNORED_PREFIXES: &[&str] = &[
    "cd ",
    "cd\t",
    "echo ",
    "printf ",
    "export ",
    "source ",
    "mkdir ",
    "rm ",
    "mv ",
    "cp ",
    "chmod ",
    "chown ",
    "touch ",
    "which ",
    "type ",
    "test ",
    "true",
    "false",
    "sleep ",
    "wait",
    "kill ",
    "set ",
    "unset ",
    "sort ",
    "uniq ",
    "tr ",
    "cut ",
    "awk ",
    "sed ",
    "python3 -c",
    "python -c",
    "node -e",
    "ruby -e",
    "atl ",
    "pwd",
    "bash ",
    "sh ",
    "then\n",
    "then ",
    "else\n",
    "else ",
    "do\n",
    "do ",
    "for ",
    "while ",
    "if ",
    "case ",
];

pub const IGNORED_EXACT: &[&str] = &[
    "cd", "echo", "true", "false", "wait", "pwd", "bash", "sh", "fi", "done",
];
