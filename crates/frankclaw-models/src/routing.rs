//! Smart model routing based on task complexity.
//!
//! Uses a 13-dimension complexity scorer to analyze prompts and route simple
//! queries to a cheaper/faster model while reserving the primary model for
//! complex tasks. This can reduce costs by 50-70% for typical usage.
//!
//! Derived from IronClaw (MIT OR Apache-2.0, Copyright (c) 2024-2025 NEAR AI Inc.)

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Complexity tiers
// ---------------------------------------------------------------------------

/// Complexity tier produced by the 13-dimension scorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Tier {
    /// Simple requests: greetings, quick lookups (score 0-15).
    Flash,
    /// Standard tasks: writing, comparisons (score 16-40).
    Standard,
    /// Complex work: multi-step analysis, code review (score 41-65).
    Pro,
    /// Critical tasks: security audits, high-stakes decisions (score 66+).
    Frontier,
}

impl Tier {
    pub fn from_score(score: u32) -> Self {
        match score {
            0..=15 => Self::Flash,
            16..=40 => Self::Standard,
            41..=65 => Self::Pro,
            _ => Self::Frontier,
        }
    }
}

/// Classification of a request's complexity, determining which model handles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// Short, simple queries → cheap model (Flash + Standard tiers).
    Simple,
    /// Ambiguous complexity → cheap model first, cascade to primary if uncertain (Pro tier).
    Moderate,
    /// Code generation, analysis, multi-step reasoning → primary model (Frontier tier).
    Complex,
}

impl From<Tier> for TaskComplexity {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::Flash | Tier::Standard => Self::Simple,
            Tier::Pro => Self::Moderate,
            Tier::Frontier => Self::Complex,
        }
    }
}

// ---------------------------------------------------------------------------
// Scorer weights
// ---------------------------------------------------------------------------

/// Weights for each of the 13 scoring dimensions.
#[derive(Debug, Clone)]
pub struct ScorerWeights {
    pub reasoning_words: f32,
    pub token_estimate: f32,
    pub code_indicators: f32,
    pub multi_step: f32,
    pub domain_specific: f32,
    pub ambiguity: f32,
    pub creativity: f32,
    pub precision: f32,
    pub context_dependency: f32,
    pub tool_likelihood: f32,
    pub safety_sensitivity: f32,
    pub question_complexity: f32,
    pub sentence_complexity: f32,
}

impl Default for ScorerWeights {
    fn default() -> Self {
        Self {
            reasoning_words: 0.14,
            token_estimate: 0.12,
            code_indicators: 0.10,
            multi_step: 0.10,
            domain_specific: 0.10,
            ambiguity: 0.05,
            creativity: 0.07,
            precision: 0.06,
            context_dependency: 0.05,
            tool_likelihood: 0.05,
            safety_sensitivity: 0.04,
            question_complexity: 0.07,
            sentence_complexity: 0.05,
        }
    }
}

/// Default domain-specific keywords for complexity scoring.
pub const DEFAULT_DOMAIN_KEYWORDS: &[&str] = &[
    // Infrastructure
    "kubernetes", "k8s", "docker", "terraform", "nginx", "apache",
    "linux", "unix", "bash", "shell",
    // Languages & frameworks
    "solidity", "rust", "typescript", "react", "nextjs", "vue", "angular", "svelte",
    // Databases
    "postgresql", "postgres", "mysql", "mongodb", "redis",
    // APIs & protocols
    "graphql", "grpc", "protobuf", "websocket", "oauth", "jwt",
    "cors", "csrf", "xss", "sql.?injection", "api", "rest",
    // Cloud & deployment
    "aws", "gcp", "azure", "vercel", "netlify", "cloudflare",
    "ci/cd", "devops",
    // Version control
    "git", "github", "gitlab",
    // Blockchain
    "blockchain", "web3", "defi", "nft", "smart.?contract",
    "ethereum", "evm",
];

/// Configuration for the complexity scorer.
#[derive(Debug, Clone, Default)]
pub struct ScorerConfig {
    pub weights: ScorerWeights,
    /// Custom domain-specific keywords (overrides defaults if provided).
    pub domain_keywords: Option<Vec<String>>,
}

/// Breakdown of complexity score by dimension.
#[derive(Debug, Clone)]
pub struct ScoreBreakdown {
    /// Total complexity score (0-100).
    pub total: u32,
    /// Computed tier.
    pub tier: Tier,
    /// Per-dimension scores (0-100 each).
    pub components: HashMap<String, u32>,
    /// Human-readable hints about why this score.
    pub hints: Vec<String>,
}

// ---------------------------------------------------------------------------
// Static regex patterns (compiled once via LazyLock)
// ---------------------------------------------------------------------------

static RE_REASONING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(why|how|explain|analyze|analyse|compare|contrast|evaluate|assess|reason|think|consider|implications?|consequences?|trade-?offs?|pros?\s*(and|&)\s*cons?|advantages?|disadvantages?|benefits?|drawbacks?|differs?|difference|versus|vs\.?|better|worse|optimal|best|worst)\b"
    ).expect("RE_REASONING is a valid regex")
});

static RE_MULTI_STEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(first|then|next|after|before|finally|step|steps|phase|stages?|process|workflow|sequence|procedure|pipeline|chain|series|order|followed by)\b"
    ).expect("RE_MULTI_STEP is a valid regex")
});

static RE_CREATIVITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(write|create|generate|compose|design|imagine|brainstorm|ideate|draft|invent|story|poem|essay|article|blog|content|narrative|script|summarize|summarise|rewrite|paraphrase|translate|adapt|tweet|post|thread|outline|structure|format|style|tone|voice)\b"
    ).expect("RE_CREATIVITY is a valid regex")
});

static RE_PRECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(\d{4}|\d+\.\d+|exactly|precisely|specific|accurate|correct|verify|confirm|date|time|number|calculate|compute|measure|count)\b"
    ).expect("RE_PRECISION is a valid regex")
});

static RE_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(`{1,3}|```|function|const|let|var|import|export|class|def |async|await|=>|\.ts|\.js|\.py|\.rs|\.go|\.sol|\(\)|\[\]|\{\}|<[A-Z][a-z]+>|useState|useEffect|npm|yarn|pnpm|cargo|pip|implement|rebase|merge|commit|branch|PR|pull.?request|columns?|migrations?|module|refactor|debug|fix|bug|error|schema|database|query)"
    ).expect("RE_CODE is a valid regex")
});

static RE_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(file|read|write|search|fetch|run|execute|check|look up|find|open|save|send|post|get|download|upload|install|deploy|build|compile|test|add|update|remove|delete|modify|change|edit|create|resolve|push|pull|clone)\b"
    ).expect("RE_TOOL is a valid regex")
});

static RE_SAFETY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(password|secret|private|confidential|medical|legal|financial|personal|sensitive|ssn|credit.?card|auth|token|key|encrypt|decrypt|hash|vulnerability|exploit|attack|breach)\b"
    ).expect("RE_SAFETY is a valid regex")
});

static RE_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(previous|earlier|above|before|last|that|those|it|they|we discussed|you said|mentioned|remember|recall|as I said|like I mentioned)\b"
    ).expect("RE_CONTEXT is a valid regex")
});

static RE_VAGUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(it|this|that|something|stuff|thing|things)\b")
        .expect("RE_VAGUE is a valid regex")
});

static RE_OPEN_ENDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(why|how|what if|explain|describe|elaborate|discuss)\b")
        .expect("RE_OPEN_ENDED is a valid regex")
});

static RE_CONJUNCTIONS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(and|but|or|however|therefore|because|although|while|whereas|moreover|furthermore)\b",
    )
    .expect("RE_CONJUNCTIONS is a valid regex")
});

static RE_TIER_HINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\[tier:(flash|standard|pro|frontier)\]")
        .expect("RE_TIER_HINT is a valid regex")
});

/// Default domain regex, compiled once from `DEFAULT_DOMAIN_KEYWORDS`.
static RE_DOMAIN_DEFAULT: LazyLock<Regex> =
    LazyLock::new(|| build_domain_regex(DEFAULT_DOMAIN_KEYWORDS));

// ---------------------------------------------------------------------------
// Pattern overrides (fast-path before scoring)
// ---------------------------------------------------------------------------

struct PatternOverride {
    regex: Regex,
    tier: Tier,
}

static DEFAULT_OVERRIDES: LazyLock<Vec<PatternOverride>> = LazyLock::new(|| {
    vec![
        // Flash tier: greetings and acknowledgments
        PatternOverride {
            regex: Regex::new(
                "(?i)^(hi|hello|hey|thanks|ok|sure|yes|no|yep|nope|cool|nice|great|got it)$",
            )
            .expect("greeting pattern is valid"),
            tier: Tier::Flash,
        },
        // Flash tier: quick lookups (end-anchored to avoid "What time complexity...")
        PatternOverride {
            regex: Regex::new(
                r"(?i)^what(?:'s|\s+is)?\s+(?:the\s+)?(time|date|day|weather)\b(?:\s+(?:is\s+it|today|now|in\s+\S+))?[?.!]*$",
            )
            .expect("lookup pattern is valid"),
            tier: Tier::Flash,
        },
        // Frontier tier: security audits
        PatternOverride {
            regex: Regex::new("(?i)security.*(audit|review|scan)")
                .expect("security audit pattern is valid"),
            tier: Tier::Frontier,
        },
        PatternOverride {
            regex: Regex::new("(?i)vulnerabilit(y|ies).*(review|scan|check|audit)")
                .expect("vulnerability pattern is valid"),
            tier: Tier::Frontier,
        },
        // Pro tier: production deployments
        PatternOverride {
            regex: Regex::new("(?i)deploy.*(mainnet|production)")
                .expect("deploy pattern is valid"),
            tier: Tier::Pro,
        },
        PatternOverride {
            regex: Regex::new("(?i)production.*(deploy|release|push)")
                .expect("production pattern is valid"),
            tier: Tier::Pro,
        },
    ]
});

// ---------------------------------------------------------------------------
// Scoring functions
// ---------------------------------------------------------------------------

fn count_matches(re: &Regex, text: &str) -> usize {
    re.find_iter(text).count()
}

fn build_domain_regex(keywords: &[&str]) -> Regex {
    if keywords.is_empty() {
        return RE_DOMAIN_DEFAULT.clone();
    }
    let pattern = format!(r"(?i)\b({})\b", keywords.join("|"));
    Regex::new(&pattern).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Invalid domain keywords pattern, using minimal fallback");
        Regex::new(r"(?i)\b(api|code|deploy)\b").expect("fallback regex is valid")
    })
}

/// Score a prompt's complexity across 13 dimensions using default config.
pub fn score_complexity(prompt: &str) -> ScoreBreakdown {
    score_complexity_with_config(prompt, &ScorerConfig::default())
}

/// Score with custom configuration (weights + domain keywords).
pub fn score_complexity_with_config(prompt: &str, config: &ScorerConfig) -> ScoreBreakdown {
    let domain_regex = match &config.domain_keywords {
        Some(custom) => {
            let refs: Vec<&str> = custom.iter().map(std::string::String::as_str).collect();
            build_domain_regex(&refs)
        }
        None => RE_DOMAIN_DEFAULT.clone(),
    };
    score_complexity_internal(prompt, &config.weights, &domain_regex)
}

#[expect(clippy::too_many_lines, reason = "13-dimension scorer; splitting would scatter related scoring logic")]
fn score_complexity_internal(
    prompt: &str,
    weights: &ScorerWeights,
    domain_regex: &Regex,
) -> ScoreBreakdown {
    let mut hints = Vec::new();
    let mut components = HashMap::new();

    // Check for explicit tier hint (e.g. "[tier:flash]")
    if let Some(caps) = RE_TIER_HINT.captures(prompt) {
        let tier_str = caps.get(1).expect("capture group 1 exists").as_str();
        let tier = match tier_str.to_lowercase().as_str() {
            "flash" => Tier::Flash,
            "standard" => Tier::Standard,
            "pro" => Tier::Pro,
            "frontier" => Tier::Frontier,
            _ => Tier::Standard,
        };
        hints.push(format!("Explicit tier hint: {tier}"));
        let total = match tier {
            Tier::Flash => 8,
            Tier::Standard => 28,
            Tier::Pro => 52,
            Tier::Frontier => 80,
        };
        return ScoreBreakdown {
            total,
            tier,
            components,
            hints,
        };
    }

    // Token estimate (based on char count): <20 chars = 0, >=520 chars = 100
    let char_count = prompt.len();
    let token_score = ((char_count as i32 - 20).max(0) as f32 / 5.0).min(100.0) as u32;
    components.insert("token_estimate".to_string(), token_score);
    if char_count > 200 {
        hints.push(format!("Long prompt ({char_count} chars)"));
    }

    // Reasoning words
    let reasoning_count = count_matches(&RE_REASONING, prompt);
    let reasoning_score = (reasoning_count * 50).min(100) as u32;
    components.insert("reasoning_words".to_string(), reasoning_score);
    if reasoning_count >= 2 {
        hints.push(format!("reasoning_words: {reasoning_count} matches"));
    }

    // Multi-step
    let multi_step_count = count_matches(&RE_MULTI_STEP, prompt);
    let multi_step_score = (multi_step_count * 50).min(100) as u32;
    components.insert("multi_step".to_string(), multi_step_score);
    if multi_step_count >= 2 {
        hints.push(format!("multi_step: {multi_step_count} matches"));
    }

    // Creativity
    let creativity_count = count_matches(&RE_CREATIVITY, prompt);
    let creativity_score = (creativity_count * 50).min(100) as u32;
    components.insert("creativity".to_string(), creativity_score);
    if creativity_count >= 2 {
        hints.push(format!("creativity: {creativity_count} matches"));
    }

    // Precision
    let precision_count = count_matches(&RE_PRECISION, prompt);
    let precision_score = (precision_count * 50).min(100) as u32;
    components.insert("precision".to_string(), precision_score);

    // Code indicators
    let code_count = count_matches(&RE_CODE, prompt);
    let code_score = (code_count * 50).min(100) as u32;
    components.insert("code_indicators".to_string(), code_score);
    if code_count >= 2 {
        hints.push(format!("code_indicators: {code_count} matches"));
    }

    // Tool likelihood
    let tool_count = count_matches(&RE_TOOL, prompt);
    let tool_score = (tool_count * 50).min(100) as u32;
    components.insert("tool_likelihood".to_string(), tool_score);

    // Safety sensitivity
    let safety_count = count_matches(&RE_SAFETY, prompt);
    let safety_score = (safety_count * 50).min(100) as u32;
    components.insert("safety_sensitivity".to_string(), safety_score);
    if safety_count >= 1 {
        hints.push(format!("safety_sensitivity: {safety_count} matches"));
    }

    // Context dependency
    let context_count = count_matches(&RE_CONTEXT, prompt);
    let context_score = (context_count * 50).min(100) as u32;
    components.insert("context_dependency".to_string(), context_score);

    // Domain specific
    let domain_count = count_matches(domain_regex, prompt);
    let domain_score = (domain_count * 50).min(100) as u32;
    components.insert("domain_specific".to_string(), domain_score);
    if domain_count >= 2 {
        hints.push(format!("domain_specific: {domain_count} matches"));
    }

    // Ambiguity (vague pronouns)
    let vague_count = count_matches(&RE_VAGUE, prompt);
    let ambiguity_score = (vague_count * 25).min(100) as u32;
    components.insert("ambiguity".to_string(), ambiguity_score);

    // Question complexity
    let question_marks = prompt.matches('?').count();
    let open_ended_count = count_matches(&RE_OPEN_ENDED, prompt);
    let question_score = ((question_marks * 20) + (open_ended_count * 25)).min(100) as u32;
    components.insert("question_complexity".to_string(), question_score);
    if question_marks >= 2 {
        hints.push(format!("Multiple questions: {question_marks}"));
    }

    // Sentence complexity (commas, semicolons, conjunctions)
    let commas = prompt.matches(',').count();
    let semicolons = prompt.matches(';').count();
    let conjunctions = count_matches(&RE_CONJUNCTIONS, prompt);
    let clauses = commas + (semicolons * 2) + conjunctions;
    let sentence_score = (clauses * 12).min(100) as u32;
    components.insert("sentence_complexity".to_string(), sentence_score);
    if clauses >= 5 {
        hints.push(format!("Complex structure: {clauses} clauses"));
    }

    // Calculate weighted total
    let total: f32 = [
        ("reasoning_words", weights.reasoning_words),
        ("token_estimate", weights.token_estimate),
        ("code_indicators", weights.code_indicators),
        ("multi_step", weights.multi_step),
        ("domain_specific", weights.domain_specific),
        ("ambiguity", weights.ambiguity),
        ("creativity", weights.creativity),
        ("precision", weights.precision),
        ("context_dependency", weights.context_dependency),
        ("tool_likelihood", weights.tool_likelihood),
        ("safety_sensitivity", weights.safety_sensitivity),
        ("question_complexity", weights.question_complexity),
        ("sentence_complexity", weights.sentence_complexity),
    ]
    .iter()
    .map(|(name, weight)| components.get(*name).copied().unwrap_or(0) as f32 * weight)
    .sum();

    // Multi-dimensional boost: +30% when 3+ dimensions fire above threshold
    let triggered_dimensions = components.values().filter(|&&v| v > 20).count();
    let total = if triggered_dimensions >= 3 {
        hints.push(format!(
            "Multi-dimensional ({triggered_dimensions} triggers)"
        ));
        total * 1.3
    } else if triggered_dimensions >= 2 {
        total * 1.15
    } else {
        total
    };

    let total = (total as u32).clamp(0, 100);
    let tier = Tier::from_score(total);

    ScoreBreakdown {
        total,
        tier,
        components,
        hints,
    }
}

/// Classify a user message for routing, checking pattern overrides first.
pub fn classify_message(message: &str) -> TaskComplexity {
    let message = message.trim();

    // Highest priority: explicit tier hints
    if let Some(caps) = RE_TIER_HINT.captures(message) {
        let tier_str = caps.get(1).expect("capture group 1 exists").as_str();
        let tier = match tier_str.to_lowercase().as_str() {
            "flash" => Tier::Flash,
            "standard" => Tier::Standard,
            "pro" => Tier::Pro,
            "frontier" => Tier::Frontier,
            _ => Tier::Standard,
        };
        return TaskComplexity::from(tier);
    }

    // Fast-path: check pattern overrides
    for po in DEFAULT_OVERRIDES.iter() {
        if po.regex.is_match(message) {
            return TaskComplexity::from(po.tier);
        }
    }

    // Full 13-dimension scoring
    let breakdown = score_complexity(message);
    TaskComplexity::from(breakdown.tier)
}

/// Check if a response from a cheap model shows uncertainty, warranting escalation.
pub fn response_is_uncertain(content: &str) -> bool {
    let content = content.trim();
    if content.is_empty() {
        return true;
    }

    let lower = content.to_lowercase();
    let uncertainty_patterns = [
        "i'm not sure",
        "i am not sure",
        "i don't know",
        "i do not know",
        "i'm unable to",
        "i am unable to",
        "i cannot",
        "i can't",
        "beyond my capabilities",
        "beyond my ability",
        "i'm not able to",
        "i am not able to",
        "i don't have enough",
        "i do not have enough",
        "i need more context",
        "i need more information",
        "could you clarify",
        "could you provide more",
        "i'm not confident",
        "i am not confident",
    ];

    uncertainty_patterns.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // -----------------------------------------------------------------------
    // Tier boundaries
    // -----------------------------------------------------------------------

    #[rstest]
    #[case(0, Tier::Flash)]
    #[case(15, Tier::Flash)]
    #[case(16, Tier::Standard)]
    #[case(40, Tier::Standard)]
    #[case(41, Tier::Pro)]
    #[case(65, Tier::Pro)]
    #[case(66, Tier::Frontier)]
    #[case(100, Tier::Frontier)]
    fn tier_from_score_boundaries(#[case] score: u32, #[case] expected: Tier) {
        assert_eq!(Tier::from_score(score), expected);
    }

    #[test]
    fn tier_display() {
        assert_eq!(Tier::Flash.to_string(), "flash");
        assert_eq!(Tier::Frontier.to_string(), "frontier");
        let s: &'static str = Tier::Standard.into();
        assert_eq!(s, "standard");
    }

    #[rstest]
    #[case(Tier::Flash, TaskComplexity::Simple)]
    #[case(Tier::Standard, TaskComplexity::Simple)]
    #[case(Tier::Pro, TaskComplexity::Moderate)]
    #[case(Tier::Frontier, TaskComplexity::Complex)]
    fn tier_to_task_complexity(#[case] tier: Tier, #[case] expected: TaskComplexity) {
        assert_eq!(TaskComplexity::from(tier), expected);
    }

    // -----------------------------------------------------------------------
    // Score complexity: basic tiers
    // -----------------------------------------------------------------------

    #[test]
    fn score_empty_prompt_is_flash() {
        let result = score_complexity("");
        assert_eq!(result.tier, Tier::Flash);
        assert!(result.total <= 15);
    }

    #[test]
    fn score_simple_greeting_is_flash() {
        let result = score_complexity("Hi");
        assert_eq!(result.tier, Tier::Flash);
        assert!(result.total <= 15);
    }

    #[test]
    fn score_quick_question_is_flash_or_standard() {
        let result = score_complexity("What time is it?");
        assert!(
            result.tier == Tier::Flash || result.tier == Tier::Standard,
            "Expected Flash or Standard, got {:?} (score {})",
            result.tier,
            result.total
        );
    }

    #[test]
    fn score_code_task_is_standard_or_higher() {
        let result = score_complexity("Implement a function to sort an array in TypeScript");
        assert!(
            result.tier == Tier::Standard || result.tier == Tier::Pro,
            "Expected Standard or Pro, got {:?} (score {})",
            result.tier,
            result.total
        );
    }

    #[test]
    fn score_complex_analysis_is_at_least_standard() {
        let result = score_complexity(
            "Explain why React uses a virtual DOM and compare it to Svelte's approach. \
             Consider the trade-offs for performance and developer experience.",
        );
        assert!(result.total >= 20, "Expected score >= 20, got {}", result.total);
    }

    #[test]
    fn score_security_audit_prompt_is_at_least_standard() {
        let result = score_complexity(
            "Analyze this Solidity contract for reentrancy vulnerabilities, \
             check for authentication bypass, and provide a security audit report.",
        );
        assert!(result.total >= 16, "Expected score >= 16, got {}", result.total);
    }

    // -----------------------------------------------------------------------
    // Individual dimensions (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("Why is this better? Explain the trade-offs and compare", "reasoning_words", 100)]
    #[case("First, read the file. Then analyze. After that, write a report.", "multi_step", 100)]
    #[case("Fix the bug in the async function, refactor the module", "code_indicators", 50)]
    #[case("Store the password and encrypt the auth token", "safety_sensitivity", 100)]
    #[case("Deploy the kubernetes cluster on aws with terraform", "domain_specific", 100)]
    #[case("Write a blog post about design patterns, then summarize", "creativity", 100)]
    fn score_dimension(#[case] prompt: &str, #[case] dimension: &str, #[case] min_score: u32) {
        let result = score_complexity(prompt);
        let score = result.components.get(dimension).copied().unwrap_or(0);
        assert!(score >= min_score, "Expected {dimension} >= {min_score}, got {score}");
    }

    #[test]
    fn score_multi_step_dimension_hint() {
        let result = score_complexity(
            "First, read the file at src/auth.ts. Then analyze it for security issues. \
             After that, write a detailed report.",
        );
        let multi_step = result.components.get("multi_step").copied().unwrap_or(0);
        assert!(multi_step >= 100, "Expected multi_step >= 100, got {multi_step}");
        assert!(result.hints.iter().any(|h| h.contains("multi_step")));
    }

    #[test]
    fn score_question_complexity_dimension() {
        let result = score_complexity("Why does this fail? How can I fix it? What if I try X?");
        let qc = result.components.get("question_complexity").copied().unwrap_or(0);
        assert!(qc >= 60, "Expected question_complexity >= 60, got {qc}");
        assert!(result.hints.iter().any(|h| h.contains("Multiple questions")));
    }

    #[test]
    fn score_sentence_complexity_dimension() {
        let result = score_complexity(
            "This is complex, because it has commas, and conjunctions, \
             however it also has semicolons; moreover, it keeps going, and going",
        );
        let sc = result.components.get("sentence_complexity").copied().unwrap_or(0);
        assert!(sc >= 60, "Expected sentence_complexity >= 60, got {sc}");
    }

    #[test]
    fn score_token_estimate_for_long_prompt() {
        let long_prompt = "a ".repeat(300); // 600 chars
        let result = score_complexity(&long_prompt);
        let token = result.components.get("token_estimate").copied().unwrap_or(0);
        assert!(token >= 80, "Expected token_estimate >= 80, got {token}");
    }

    #[test]
    fn score_token_estimate_for_short_prompt() {
        let result = score_complexity("hi");
        let token = result.components.get("token_estimate").copied().unwrap_or(0);
        assert_eq!(token, 0, "Expected token_estimate == 0, got {token}");
    }

    // -----------------------------------------------------------------------
    // Multi-dimensional boost
    // -----------------------------------------------------------------------

    #[test]
    fn score_multi_dimensional_boost() {
        let result = score_complexity(
            "First, explain why the kubernetes deployment fails. \
             Then refactor the auth module to fix the vulnerability. \
             After that, write a security report comparing the approaches.",
        );
        assert!(
            result.hints.iter().any(|h| h.contains("Multi-dimensional")),
            "Expected multi-dimensional boost, hints: {:?}",
            result.hints
        );
    }

    // -----------------------------------------------------------------------
    // Explicit tier hints (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("[tier:flash] This looks complex but override to flash", Tier::Flash)]
    #[case("[tier:frontier] Simple question but I want the best", Tier::Frontier)]
    #[case("[tier:PRO] some message", Tier::Pro)]
    fn score_explicit_tier_hint(#[case] prompt: &str, #[case] expected: Tier) {
        let result = score_complexity(prompt);
        assert_eq!(result.tier, expected);
        assert!(result.hints.iter().any(|h| h.contains("Explicit tier hint")));
    }

    // -----------------------------------------------------------------------
    // Custom domain keywords
    // -----------------------------------------------------------------------

    #[test]
    fn score_custom_domain_keywords_override_defaults() {
        let default_result = score_complexity("How do I deploy kubernetes?");
        let default_domain = default_result.components.get("domain_specific").copied().unwrap_or(0);
        assert!(default_domain > 0, "Default keywords should match 'kubernetes'");

        let config = ScorerConfig {
            weights: ScorerWeights::default(),
            domain_keywords: Some(vec!["mycompany".to_string(), "myproduct".to_string()]),
        };
        let custom_result = score_complexity_with_config("How do I deploy kubernetes?", &config);
        let custom_domain = custom_result.components.get("domain_specific").copied().unwrap_or(0);
        assert_eq!(custom_domain, 0, "Custom keywords shouldn't match 'kubernetes'");

        let custom_result2 = score_complexity_with_config("Tell me about myproduct features", &config);
        let custom_domain2 = custom_result2.components.get("domain_specific").copied().unwrap_or(0);
        assert!(custom_domain2 > 0, "Custom keywords should match 'myproduct'");
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn score_whitespace_only_is_flash() {
        let result = score_complexity("   \n\t  ");
        assert_eq!(result.tier, Tier::Flash);
    }

    #[test]
    fn score_single_word_no_keywords() {
        let result = score_complexity("banana");
        assert!(
            result.tier == Tier::Flash || result.tier == Tier::Standard,
            "Single non-keyword word should be Flash or Standard, got {:?}",
            result.tier
        );
    }

    #[test]
    fn score_very_long_prompt_is_at_least_standard() {
        let long = "Tell me about ".to_string() + &"things ".repeat(200);
        let result = score_complexity(&long);
        assert!(result.total >= 16, "Very long prompt should score at least Standard, got {}", result.total);
    }

    #[test]
    fn score_all_dimensions_have_entries() {
        let result = score_complexity(
            "First, explain why the function fails. Then write a fix and deploy it.",
        );
        let expected_keys = [
            "reasoning_words", "token_estimate", "code_indicators", "multi_step",
            "domain_specific", "ambiguity", "creativity", "precision",
            "context_dependency", "tool_likelihood", "safety_sensitivity",
            "question_complexity", "sentence_complexity",
        ];
        for key in &expected_keys {
            assert!(result.components.contains_key(*key), "Missing component: {key}");
        }
    }

    #[test]
    fn score_is_clamped_to_100() {
        let prompt = "First, explain why the kubernetes docker terraform deployment on aws fails. \
             Then analyze the security vulnerability and compare the trade-offs. \
             After that, write a detailed blog post report with code examples: \
             ```rust\nfn main() {}\n``` \
             Calculate exactly how many steps are needed? Why? How? \
             Deploy to production mainnet. Review the authentication token password.";
        let result = score_complexity(prompt);
        assert!(result.total <= 100, "Score should be clamped to 100, got {}", result.total);
    }

    // -----------------------------------------------------------------------
    // Pattern overrides via classify_message (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("Hi", TaskComplexity::Simple)]
    #[case("hello", TaskComplexity::Simple)]
    #[case("thanks", TaskComplexity::Simple)]
    #[case("Please do a security audit of this contract", TaskComplexity::Complex)]
    #[case("Deploy this to production", TaskComplexity::Moderate)]
    #[case("What time is it?", TaskComplexity::Simple)]
    fn pattern_override_classification(#[case] msg: &str, #[case] expected: TaskComplexity) {
        assert_eq!(classify_message(msg), expected);
    }

    #[test]
    fn pattern_override_time_does_not_match_complex_questions() {
        let overrides = &*DEFAULT_OVERRIDES;
        let lookup_override = overrides
            .iter()
            .find(|po| po.tier == Tier::Flash && po.regex.as_str().contains("time"))
            .expect("time lookup override exists");

        assert!(
            !lookup_override.regex.is_match("What time complexity is merge sort?"),
            "Time override should not match 'What time complexity is merge sort?'"
        );
        assert!(lookup_override.regex.is_match("What time is it?"));
        assert!(lookup_override.regex.is_match("what's the date today?"));
    }

    #[test]
    fn tier_hint_overrides_pattern_override() {
        // "[tier:flash] security audit review" has a Frontier pattern override
        // but tier hints should win.
        assert_eq!(
            classify_message("[tier:flash] security audit review"),
            TaskComplexity::Simple
        );
    }

    #[test]
    fn trimmed_greeting_matches_override() {
        assert_eq!(classify_message("  hello  \n"), TaskComplexity::Simple);
    }

    #[test]
    fn empty_domain_keywords_uses_defaults() {
        let config = ScorerConfig {
            domain_keywords: Some(vec![]),
            ..ScorerConfig::default()
        };
        let result = score_complexity_with_config("deploy kubernetes to mainnet", &config);
        assert!(
            result.components.get("domain_specific").copied().unwrap_or(0) > 0,
            "Empty custom keywords should fall back to defaults"
        );
    }

    // -----------------------------------------------------------------------
    // Uncertainty detection (parameterized)
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("I'm not sure.", true)]
    #[case("I don't know how to do that.", true)]
    #[case("I cannot help with that.", true)]
    #[case("Could you clarify what you mean?", true)]
    #[case("", true)]
    #[case("   ", true)]
    #[case("Yes.", false)]
    #[case("The answer is 42.", false)]
    #[case("Deployed successfully.", false)]
    fn uncertainty_detection(#[case] response: &str, #[case] expected: bool) {
        assert_eq!(response_is_uncertain(response), expected);
    }
}
