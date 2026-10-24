#[cfg(test)]
pub const VOCAB: &[&str] = &[
    "git",
    "GitHub",
    "GitHub Actions",
    "gh",
    "CI",
    "commit",
    "push",
    "stash",
    "pull request",
    "PR",
    "merge",
    "rebase",
    "branch",
    "npm",
    "pnpm",
    "npx",
    "yarn",
    "bunx",
    "uv",
    "winget",
    "Homebrew",
    "cargo",
    "rustc",
    "Rust",
    "TypeScript",
    "JavaScript",
    "Python",
    "pip",
    "README",
    "package.json",
    "tsconfig",
    ".env",
    "JSON",
    "YAML",
    "API",
    "CLI",
    "localhost",
    "env",
    "config",
    "repo",
    "diff",
    "lint",
    "ESLint",
    "refactor",
    "React",
    "Next.js",
    "Vite",
    "Tailwind",
    "Node.js",
    "Vitest",
    "Playwright",
    "Docker",
    "Kubernetes",
    "kubectl",
    "Vercel",
    "Supabase",
    "Postgres",
    "SQLite",
    "Prisma",
    "ssh",
    "WSL",
    "ripgrep",
    "Claude",
    "Claude Code",
    "Anthropic",
    "Codex",
    "OpenAI",
    "ChatGPT",
    "Cursor",
    "Copilot",
    "Windsurf",
    "Ollama",
    "herdr",
    "MCP server",
    "subagent",
    "LLM",
    "VS Code",
    "PowerShell",
    "terminal",
    "directory",
    "sudo",
    "cd",
    "ls",
    "mkdir",
];

pub const WHISPER_PROMPT: &str = "In the terminal I cd into the repo directory, run ls, mkdir, sudo, \
    ssh and ripgrep in WSL or PowerShell, and use git to commit, push, stash, diff, merge and rebase \
    a branch, then open a pull request (PR) on GitHub with gh while GitHub Actions runs CI. I run \
    npm, pnpm, npx, yarn, bunx, uv, pip, winget, Homebrew, cargo and rustc, lint and refactor Rust, \
    TypeScript, JavaScript and Python with ESLint, and edit the README, package.json, tsconfig, \
    .env, JSON, YAML and config files. I build with React, Next.js, Vite, Tailwind and Node.js, test \
    with Vitest and Playwright, and ship with Docker, Kubernetes, kubectl, Vercel, Supabase, \
    Postgres, SQLite and Prisma behind an API or CLI on localhost. I work with Claude, Claude Code, \
    Anthropic, Codex, OpenAI, ChatGPT, Cursor, Copilot, Windsurf and Ollama in VS Code, and run \
    herdr to herd my agents, each subagent, MCP server and LLM.";

const CORRECTIONS: &[(&str, &str)] = &[
    ("get push", "git push"),
    ("get pull", "git pull"),
    ("get commit", "git commit"),
    ("get status", "git status"),
    ("get checkout", "git checkout"),
    ("get rebase", "git rebase"),
    ("get stash", "git stash"),
    ("get clone", "git clone"),
    ("get fetch", "git fetch"),
    ("get diff", "git diff"),
    ("get merge", "git merge"),
    ("get add", "git add"),
    ("get init", "git init"),
    ("get branch", "git branch"),
    ("get cherry pick", "git cherry-pick"),
    ("git cherry pick", "git cherry-pick"),
    ("get reset --hard", "git reset --hard"),
    ("get reset hard", "git reset --hard"),
    ("git reset hard", "git reset --hard"),
    ("get work tree", "git worktree"),
    ("git work tree", "git worktree"),
    ("dot git ignore", ".gitignore"),
    ("dot get ignore", ".gitignore"),
    ("git ignore", ".gitignore"),
    ("get hub", "GitHub"),
    ("git hub", "GitHub"),
    ("GitHub actions", "GitHub Actions"),
    ("GitHub co-pilot", "GitHub Copilot"),
    ("GitHub co pilot", "GitHub Copilot"),
    ("GitHub copilot", "GitHub Copilot"),
    ("Microsoft co-pilot", "Microsoft Copilot"),
    ("P R", "PR"),
    ("G H PR", "gh pr"),
    ("G H issue", "gh issue"),
    ("G H repo", "gh repo"),
    ("G H auth", "gh auth"),
    ("C I C D", "CI/CD"),
    ("CI CD", "CI/CD"),
    ("p n p m", "pnpm"),
    ("P NPM", "pnpm"),
    ("pee npm", "pnpm"),
    ("PNP M", "pnpm"),
    ("PNPM", "pnpm"),
    ("n p m", "npm"),
    ("N PM", "npm"),
    ("and PM", "npm"),
    ("M P M", "npm"),
    ("MPM", "npm"),
    ("NPM", "npm"),
    ("N P X", "npx"),
    ("N PX", "npx"),
    ("NPX", "npx"),
    ("bun x", "bunx"),
    ("UV x", "uvx"),
    ("UVX", "uvx"),
    ("UV run", "uv run"),
    ("UV sync", "uv sync"),
    ("UV pip", "uv pip"),
    ("UV add", "uv add"),
    ("UV venv", "uv venv"),
    ("pip x", "pipx"),
    ("N V M", "nvm"),
    ("NVM", "nvm"),
    ("node JS", "Node.js"),
    ("NodeJS", "Node.js"),
    ("node.js", "Node.js"),
    ("win get install", "winget install"),
    ("win get upgrade", "winget upgrade"),
    ("win get search", "winget search"),
    ("pseudo apt", "sudo apt"),
    ("apt get", "apt-get"),
    ("car go run", "cargo run"),
    ("car go build", "cargo build"),
    ("car go test", "cargo test"),
    ("car go check", "cargo check"),
    ("car go add", "cargo add"),
    ("car go install", "cargo install"),
    ("car go clippy", "cargo clippy"),
    ("rust up update", "rustup update"),
    ("rust up toolchain", "rustup toolchain"),
    ("rust up target", "rustup target"),
    ("make dir", "mkdir"),
    ("mk dir", "mkdir"),
    ("r m dash r f", "rm -rf"),
    ("rm dash r f", "rm -rf"),
    ("rm dash rf", "rm -rf"),
    ("rm minus rf", "rm -rf"),
    ("rm -rf", "rm -rf"),
    ("C H mod", "chmod"),
    ("ch mod", "chmod"),
    ("chmod plus x", "chmod +x"),
    ("rip grep", "ripgrep"),
    ("W get", "wget"),
    ("S S H", "ssh"),
    ("W S L", "WSL"),
    ("Z S H", "zsh"),
    ("power shell", "PowerShell"),
    ("read me file", "README file"),
    ("read me dot md", "README.md"),
    ("package dot json", "package.json"),
    ("package dot Jason", "package.json"),
    ("package JSON", "package.json"),
    ("package Jason", "package.json"),
    ("TS config dot json", "tsconfig.json"),
    ("tsconfig dot json", "tsconfig.json"),
    ("TS config", "tsconfig"),
    ("dot env", ".env"),
    ("Jason file", "JSON file"),
    ("docker file", "Dockerfile"),
    ("doctor compose", "docker compose"),
    ("docker compose", "docker compose"),
    ("cube control", "kubectl"),
    ("cube cuddle", "kubectl"),
    ("cube CTL", "kubectl"),
    ("kube control", "kubectl"),
    ("kube cuddle", "kubectl"),
    ("kube CTL", "kubectl"),
    ("kubectl", "kubectl"),
    ("Cooper Netties", "Kubernetes"),
    ("Cooper Nettys", "Kubernetes"),
    ("Kuber Netties", "Kubernetes"),
    ("ver cell", "Vercel"),
    ("Versel", "Vercel"),
    ("Vercell", "Vercel"),
    ("net lify", "Netlify"),
    ("super base", "Supabase"),
    ("supa base", "Supabase"),
    ("Superbase", "Supabase"),
    ("fire base", "Firebase"),
    ("cloud flare", "Cloudflare"),
    ("cloud flair", "Cloudflare"),
    ("post gress QL", "PostgreSQL"),
    ("post gres QL", "PostgreSQL"),
    ("Postgres QL", "PostgreSQL"),
    ("postgre SQL", "PostgreSQL"),
    ("Postgres sequel", "PostgreSQL"),
    ("post gress", "Postgres"),
    ("post gres", "Postgres"),
    ("post grass", "Postgres"),
    ("Postgress", "Postgres"),
    ("sequel lite", "SQLite"),
    ("sequel light", "SQLite"),
    ("SQL lite", "SQLite"),
    ("SQL light", "SQLite"),
    ("Prizma", "Prisma"),
    ("engine X", "nginx"),
    ("next JS", "Next.js"),
    ("NextJS", "Next.js"),
    ("next.js", "Next.js"),
    ("Veet", "Vite"),
    ("tail wind CSS", "Tailwind CSS"),
    ("tail wind config", "Tailwind config"),
    ("tailwind CSS", "Tailwind CSS"),
    ("E S lint", "ESLint"),
    ("ES lint", "ESLint"),
    ("type script", "TypeScript"),
    ("java script", "JavaScript"),
    ("V test", "Vitest"),
    ("vee test", "Vitest"),
    ("play wright", "Playwright"),
    ("Claude dot MD", "CLAUDE.md"),
    ("cloud dot MD", "CLAUDE.md"),
    ("Claude MD", "CLAUDE.md"),
    ("claude.md", "CLAUDE.md"),
    ("agents dot MD", "AGENTS.md"),
    ("agents.md", "AGENTS.md"),
    ("clod code", "Claude Code"),
    ("cloud code", "Claude Code"),
    ("claude coat", "Claude Code"),
    ("clawed code", "Claude Code"),
    ("claude code", "Claude Code"),
    ("cloud opus", "Claude Opus"),
    ("cloud sonnet", "Claude Sonnet"),
    ("cloud haiku", "Claude Haiku"),
    ("Claud", "Claude"),
    ("and tropic", "Anthropic"),
    ("an tropic", "Anthropic"),
    ("codecs", "Codex"),
    ("open AI", "OpenAI"),
    ("chat G P T", "ChatGPT"),
    ("chat GPT", "ChatGPT"),
    ("chat GBT", "ChatGPT"),
    ("open wind surf", "open Windsurf"),
    ("wind surf IDE", "Windsurf IDE"),
    ("wind surf editor", "Windsurf editor"),
    ("O llama", "Ollama"),
    ("oh llama", "Ollama"),
    ("Olama", "Ollama"),
    ("hugging face", "Hugging Face"),
    ("herd r", "herdr"),
    ("herder", "herdr"),
    ("M C P", "MCP"),
    ("L L Ms", "LLMs"),
    ("L L M", "LLM"),
    ("sub agents", "subagents"),
    ("sub agent", "subagent"),
    ("sub-agents", "subagents"),
    ("sub-agent", "subagent"),
    ("V S code", "VS Code"),
    ("V.S. code", "VS Code"),
    ("vs. code", "VS Code"),
    ("VS code", "VS Code"),
    ("local host", "localhost"),
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '_' || c == '-'
}

fn replace_phrase(s: &str, from: &str, to: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat = from.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(off) = lower[i..].find(&pat) {
        let start = i + off;
        let end = start + pat.len();
        let before_ok = s[..start]
            .chars()
            .next_back()
            .map_or(true, |c| !is_word_char(c));
        let after_ok = s[end..].chars().next().map_or(true, |c| !is_word_char(c));
        if before_ok && after_ok {
            out.push_str(&s[i..start]);
            out.push_str(to);
            i = end;
        } else {
            let step = start + s[start..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&s[i..step]);
            i = step;
        }
    }
    out.push_str(&s[i..]);
    out
}

pub fn fix_coding_terms(text: &str) -> String {
    CORRECTIONS.iter().fold(text.to_string(), |s, (from, to)| {
        replace_phrase(&s, from, to)
    })
}

use crate::config::Vocabulary;

impl Vocabulary {
    pub fn prompt(&self) -> String {
        let mut parts = Vec::new();
        if self.coding {
            parts.push(WHISPER_PROMPT.to_string());
        }
        if !self.words.is_empty() {
            parts.push(format!("Words: {}.", self.words.join(", ")));
        }
        parts.join(" ")
    }

    pub fn fix(&self, text: &str) -> String {
        let base = if self.coding {
            fix_coding_terms(text)
        } else {
            text.to_string()
        };
        self.words
            .iter()
            .fold(base, |s, w| replace_phrase(&s, w, w))
    }
}

pub fn current() -> Vocabulary {
    crate::config::vocabulary()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_vocabulary;

    #[test]
    fn fixes_coding_mishearings_and_leaves_speech_alone() {
        let cases = [
            ("get push to main", "git push to main"),
            ("Get status.", "git status."),
            (
                "then get commit and get pull",
                "then git commit and git pull",
            ),
            ("open it on get hub", "open it on GitHub"),
            ("run and PM install", "run npm install"),
            ("run N PM install", "run npm install"),
            ("run n p m test", "run npm test"),
            ("P NPM install", "pnpm install"),
            ("car go build please", "cargo build please"),
            ("open the read me file", "open the README file"),
            ("save a Jason file", "save a JSON file"),
            ("ask cloud code to fix it", "ask Claude Code to fix it"),
            ("clod code", "Claude Code"),
            ("start codecs", "start Codex"),
            ("open vs. code", "open VS Code"),
            ("serve on local host 3000", "serve on localhost 3000"),
            ("get stash then get fetch", "git stash then git fetch"),
            ("get clone the repo", "git clone the repo"),
            ("get diff and get merge", "git diff and git merge"),
            ("get add dot", "git add dot"),
            ("get cherry pick that commit", "git cherry-pick that commit"),
            ("get reset hard", "git reset --hard"),
            ("add it to the git ignore", "add it to the .gitignore"),
            ("check git hub actions", "check GitHub Actions"),
            ("ask GitHub co-pilot", "ask GitHub Copilot"),
            ("G H PR create", "gh pr create"),
            ("the C I C D pipeline", "the CI/CD pipeline"),
            ("run MPM install", "run npm install"),
            ("run M P M install", "run npm install"),
            ("PNPM dev", "pnpm dev"),
            ("NPX create", "npx create"),
            ("bun x vite", "bunx vite"),
            ("UV run main.py", "uv run main.py"),
            ("pip x install ruff", "pipx install ruff"),
            ("use N V M", "use nvm"),
            ("update node JS", "update Node.js"),
            ("win get install git", "winget install git"),
            ("pseudo apt get install curl", "sudo apt-get install curl"),
            ("car go clippy", "cargo clippy"),
            ("rust up update", "rustup update"),
            ("make dir src", "mkdir src"),
            ("RM dash RF node_modules", "rm -rf node_modules"),
            ("ch mod plus x run.sh", "chmod +x run.sh"),
            ("search with rip grep", "search with ripgrep"),
            ("S S H into the box", "ssh into the box"),
            ("open W S L", "open WSL"),
            ("open power shell", "open PowerShell"),
            ("edit package dot json", "edit package.json"),
            ("fix the TS config", "fix the tsconfig"),
            ("copy the dot env file", "copy the .env file"),
            ("doctor compose up", "docker compose up"),
            ("cube control get pods", "kubectl get pods"),
            ("deploy to Cooper Netties", "deploy to Kubernetes"),
            ("push to ver cell", "push to Vercel"),
            ("use super base auth", "use Supabase auth"),
            ("a post gress QL database", "a PostgreSQL database"),
            ("use post gress", "use Postgres"),
            ("a sequel lite file", "a SQLite file"),
            ("a next JS app", "a Next.js app"),
            ("add tail wind CSS", "add Tailwind CSS"),
            ("run ES lint", "run ESLint"),
            (
                "write type script and java script",
                "write TypeScript and JavaScript",
            ),
            ("run V test", "run Vitest"),
            ("a play wright test", "a Playwright test"),
            ("read the Claude dot MD", "read the CLAUDE.md"),
            ("use cloud opus", "use Claude Opus"),
            ("ask Claud", "ask Claude"),
            ("and tropic models", "Anthropic models"),
            ("ask chat GPT", "ask ChatGPT"),
            ("use open AI", "use OpenAI"),
            ("open wind surf", "open Windsurf"),
            ("run O llama", "run Ollama"),
            ("start herder", "start herdr"),
            ("run herd r now", "run herdr now"),
            ("add an M C P server", "add an MCP server"),
            ("ask the L L M", "ask the LLM"),
            ("spawn a sub agent", "spawn a subagent"),
            ("open V S code", "open VS Code"),
        ];
        for (input, want) in cases {
            assert_eq!(fix_coding_terms(input), want, "input: {input}");
        }
        let untouched = [
            "I want to get pushed back",
            "let's get lunch",
            "we did it together",
            "can you read me the story",
            "watch the car go by",
            "get pulled over",
            "the standard PMs",
            "at 5 PM",
            "let's get lost",
            "you can get logged in",
            "the router needs to get reset",
            "a cherry pick of the best",
            "get added to the list",
            "the co-pilot landed the plane",
            "we had a strong tail wind",
            "I want to wind surf this summer",
            "UV light is harmful",
            "I home brew my own beer",
            "Klein aber fein",
            "pseudo code is fine",
            "open the next JSON file",
            "the red is nice",
            "my CD collection",
            "the cat clawed the sofa",
            "the playwright wrote a play",
            "the cloud is dark",
            "Jason called me",
            "the super bowl",
            "an apt get-together",
            "Morgen gehen wir ins Kino",
            "the herders came home",
            "we note the rest",
        ];
        for s in untouched {
            assert_eq!(fix_coding_terms(s), s, "input: {s}");
        }
    }

    #[test]
    fn parses_vocabulary_setting() {
        let coding = Vocabulary {
            coding: true,
            words: Vec::new(),
        };
        assert_eq!(parse_vocabulary(None), coding);
        assert_eq!(
            parse_vocabulary(Some("vk_mode = docked\n# vocabulary = off\n")),
            coding
        );
        assert_eq!(parse_vocabulary(Some("vocabulary = coding")), coding);
        assert_eq!(parse_vocabulary(Some("vocabulary = CODING\n")), coding);
        for off in [
            "vocabulary = off",
            "vocabulary = none",
            "vocabulary =",
            "vocabulary = , ",
        ] {
            let v = parse_vocabulary(Some(off));
            assert_eq!(v, Vocabulary::default(), "input: {off}");
            assert_eq!(v.prompt(), "");
            assert_eq!(v.fix("get push and cloud code"), "get push and cloud code");
        }

        let custom = parse_vocabulary(Some("vocabulary = herdr, MyProject, myproject"));
        assert!(!custom.coding);
        assert_eq!(custom.words, ["herdr", "MyProject"]);
        assert_eq!(custom.prompt(), "Words: herdr, MyProject.");
        assert_eq!(
            custom.fix("run Herdr on myproject"),
            "run herdr on MyProject"
        );
        assert_eq!(custom.fix("get push"), "get push");

        let mixed =
            parse_vocabulary(Some("vk_mode=docked\nvocabulary = coding, herdr, Jonas\n"));
        assert!(mixed.coding);
        assert_eq!(mixed.words, ["herdr", "Jonas"]);
        assert!(mixed.prompt().starts_with(WHISPER_PROMPT));
        assert!(mixed.prompt().ends_with(" Words: herdr, Jonas."));
        assert_eq!(mixed.fix("get push for jonas"), "git push for Jonas");
    }

    #[test]
    fn whisper_prompt_fits_and_covers_vocab() {
        assert!(
            WHISPER_PROMPT.len() <= 900,
            "prompt is {} chars",
            WHISPER_PROMPT.len()
        );
        for w in VOCAB {
            assert!(WHISPER_PROMPT.contains(w), "prompt misses {w}");
        }
    }
}
