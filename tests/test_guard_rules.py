"""Table-driven tests for the guardrails rule engine (rules G1-G6)."""
from __future__ import annotations

import sys
import unittest
from unittest import mock

from _repo import ROOT

sys.path.insert(0, str(ROOT / "plugins" / "anthology-guardrails" / "scripts"))
import guard_rules  # noqa: E402

HOME = "/home/tester"
# macOS-style home of another user, assembled so no literal user path lands in the repo.
OTHER_HOME = "/" + "Us" + "ers/someone"

# Secret fixtures are built by concatenation so scanners never flag this file.
AWS_KEY = "AKIA" + "IOSFODNN7EXAMPLE"
GH_TOKEN = "ghp" + "_abc123"
GH_PAT = "github" + "_pat_" + "abcdef"
SK_KEY = "sk" + "-" + "a1b2c3d4e5f6g7h8i9"
SLACK = "xox" + "b-123"
PRIVATE_KEY = "-----BEGIN OPENSSH " + "PRIVATE KEY-----"
PW = "pass" + "word"
PWV = "hunter2" + "xyz"


def bash(command):
    return "Bash", {"command": command}


def check(case):
    tool, tool_input = case
    return guard_rules.check(tool, tool_input, HOME)


def rule_of(case):
    result = check(case)
    return result[0] if result else None


class RuleTableTest(unittest.TestCase):
    def assert_table(self, expected_rule, cases):
        for case in cases:
            with self.subTest(rule=expected_rule, case=case):
                self.assertEqual(rule_of(case), expected_rule)

    # ---------------------------------------------------------------- blocked

    def test_g1_browser_automation_installs(self):
        self.assert_table("G1", [
            bash("npx playwright install"),
            bash("npx -y playwright install chromium"),
            bash("pnpm exec playwright test"),
            bash("npm i puppeteer"),
            bash("pip install playwright"),
            bash("brew install --cask chromium"),
            # every subcommand R6 names
            *[bash(f"npx playwright {sub}") for sub in (
                "install", "install-deps", "test", "codegen", "open", "cr", "chromium", "ff",
                "firefox", "wk", "webkit", "screenshot", "pdf")],
            bash("npx playwright@1.49.0 install"),
            bash("npx @playwright/test test"),
            bash("npx playwright-core install"),
            bash("python -m playwright install"),
            bash("python3 -m playwright install chromium"),
            bash("playwright install"),
            # puppeteer family
            bash("npx puppeteer browsers install chrome"),
            bash("npx @puppeteer/browsers install chrome@stable"),
            bash("npm install puppeteer"),
            bash("npm install --save-dev puppeteer@21.0.0"),
            bash("pnpm add puppeteer"),
            bash("yarn add puppeteer"),
            bash("bun add puppeteer"),
            bash("yarn global add puppeteer"),
            # pip family
            bash("pip3 install playwright==1.49.0"),
            bash("pip install 'playwright>=1.40'"),
            bash("python -m pip install playwright"),
            bash("uv pip install playwright"),
            bash("pipx install playwright"),
            bash("pip install -U playwright[all]"),
            # OS package managers
            bash("brew install google-chrome"),
            bash("brew install homebrew/cask/google-chrome"),
            bash("brew install ungoogled-chromium"),
            bash("brew install chromedriver"),
            bash("apt-get install -y chromium-browser"),
            bash("apt install google-chrome-stable"),
            bash("dnf install chromium"),
            bash("yum install chromium"),
            bash("snap install chromium"),
            bash("pacman -S chromium"),
            bash("pacman -Sy chromium"),
            bash("apt install playwright"),
        ])

    def test_g2_host_chrome_launch(self):
        self.assert_table("G2", [
            bash('open -a "Google Chrome" https://x.dev'),
            bash('"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless'),
            bash("chromium --remote-debugging-port=9222"),
            bash("google-chrome-stable"),
            bash("google-chrome"),
            bash("chromium-browser"),
            bash("chrome"),
            bash("chromedriver --port=9515"),
            bash("/usr/bin/google-chrome --headless"),
            bash("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            bash("open -a Chromium"),
            bash("open -a chrome"),
            bash('open -na "Google Chrome" --args --remote-debugging-port=9222'),
            bash("open -b com.google.Chrome"),
            bash("open -b org.chromium.Chromium"),
            bash("open /Applications/Google\\ Chrome.app"),
            bash("osascript -e 'tell application \"Google Chrome\" to activate'"),
            bash("osascript -e 'tell application \"Chromium\" to quit'"),
            bash("node server.js --remote-debugging-port=9222"),
            bash("chrome-headless-shell --dump-dom https://x"),
            bash("npx chrome-launcher"),
        ])

    def test_g3_profile_paths_in_shell(self):
        gdir = "~/Library/Application\\ Support/Google"
        self.assert_table("G3", [
            bash("cat ~/Library/Application\\ Support/Google/Chrome/Default/Cookies"),
            bash("tar czf x.tgz " + gdir),
            bash("find " + gdir + " -name Cookies"),
            bash("ls ~/library/application\\ support/google"),
            bash("ls ~/LIBRARY/APPLICATION\\ SUPPORT/GOOGLE"),
            bash('cat "$HOME/Library/Application Support/Google/Chrome/Default/Cookies"'),
            bash('cat "${HOME}/Library/Application Support/Google/Chrome/Default/Cookies"'),
            bash("cat $HOME/Library/Application\\ Support/Chromium/Default/Cookies"),
            bash("cat ~/.config/google-chrome/Default/Cookies"),
            bash("cat ~/.config/google-chrome-beta/Default/Cookies"),
            bash("cat ~/.config/google-chrome-unstable/Default/Cookies"),
            bash("cat ~/.config/chromium/Default/Cookies"),
            bash("ls ~/.config/chromium-browser"),
            # absolute paths: the home from `home`, and another user's
            bash(f"cat {HOME}/.config/google-chrome/Default/Cookies"),
            bash(f"cat {OTHER_HOME}/Library/Application\\ Support/Google/Chrome/Default/Cookies"),
            bash(f"tar czf x.tgz {OTHER_HOME}/.config/chromium"),
            bash("cat ~root/.config/google-chrome/Default/Cookies"),
            bash("cat ~someone/Library/Application\\ Support/Google/Chrome"),
            # the root itself, with a trailing slash, dot segments and ..
            bash("ls ~/Library/Application\\ Support/Google/"),
            bash("ls ~/Library/./Application\\ Support/Google"),
            bash("ls ~/Library/Application\\ Support/Safari/../Google"),
            bash("ls ~/Library//Application\\ Support//Google"),
            # backslashes converted to slashes
            bash("dir C:\\\\x\\\\Library\\\\Application\\ Support\\\\Google"),
            # globs
            bash("ls ~/Library/Application\\ Support/G*"),
            bash("ls ~/Library/Application\\ Support/*"),
            bash("ls ~/Library/Application\\ Support/G?ogle/Chrome"),
            bash("ls ~/Library/*/Google"),
            bash("ls ~/.config/google-*"),
            bash("ls ~/.config/google-chrome-b*"),
            bash("ls ~/.config/*/Default"),
            bash("ls ~/.config/*"),
            bash("ls ~/.config/g*"),
            # partly literal globs are not wildcard-only (review N2)
            bash("cat ~/.c*/c*/Default/Cookies"),
            bash("cat ~/.conf*/chrom*/Default/Cookies"),
            bash("cat ~/.*/*/Default/Cookies"),
            bash("cat ~/Libr*/Appl*/Goo*/Chrome/Default/Cookies"),
            bash("cat ~/L*/A*/G*/Chrome/Default/Cookies"),
            bash("cat $HOME/.c?nfig/chr?mium/Default/Cookies"),
            bash("cat ~/.[c]onfig/[c]hromium/Default/Cookies"),
            bash("cd ~ && cat .c*/c*/Default/Cookies"),
            # wildcard-only windows directly under a home directory still reach the roots (N2 ruling)
            bash("ls ~/*/*/*"),
            bash("cat ~/*/*/*/Chrome/Default/Cookies"),
            bash("ls ~/*/*"),
            bash("ls $HOME/**/*"),
            bash("ls " + OTHER_HOME + "/*/*"),
            bash("ls /home/someone/*/*"),
            bash("ls /root/*/*"),
            bash("cd ~ && ls */*/*"),
            # array elements are paths too (review N1)
            bash("files=(~/.config/chromium/Default/Cookies); cat \"${files[0]}\""),
            bash("files=(/tmp/a ~/Library/Application\\ Support/Google/Chrome/Default/Cookies); "
                 "cp \"${files[@]}\" /tmp"),
            bash("files=(\"$HOME/.config/chromium/Default/Cookies\")"),
            bash("files+=(~/.config/chromium/Default/Cookies)"),
            bash("declare -a f=(~/.config/chromium/Default/Cookies)"),
            bash("local f=(~/.config/chromium/Default/Cookies)"),
            bash("files=(\n  ~/.config/chromium/Default/Cookies\n)"),
            bash("files=( # list\n  ~/.config/chromium/Default/Cookies\n); cat \"${files[0]}\""),
            bash("files=(\n  /tmp/a # first\n  ~/.config/chromium/Default/Cookies\n)"),
            bash("files=(\n  # it's the profile\n  ~/.config/chromium/Default/Cookies\n)"),
            # -C <dir> changes directory for tar / git / make (review M4)
            bash("tar czf /tmp/p.tgz -C ~/.config chromium"),
            bash("tar xf /tmp/p.tgz --directory ~/.config google-chrome"),
            bash("git -C ~/Library/Application\\ Support ls-files Google"),
            bash("make -C ~/.config/chromium all"),
            bash("ls ~/Library/Application\\ Support/{Google,Code}"),
            bash("cat ~/.config/{git,google-chrome}/Default/Cookies"),
            bash("ls ~/{.config/chromium,.zshrc}"),
            # flag=value, colon lists, redirections, docker mounts
            bash("tool --user-data-dir=~/.config/google-chrome"),
            bash("PATH=~/.config/chromium:/usr/bin tool"),
            bash("docker run -v ~/Library/Application\\ Support/Google:/data img"),
            bash("cat < ~/.config/google-chrome/Default/Cookies"),
            bash("echo hi > ~/.config/chromium/x"),
            bash("cp ~/.config/google-chrome/Default/Cookies /tmp/c"),
            bash("dd if=~/.config/google-chrome/Default/Cookies of=/tmp/c"),
            # chains, wrappers, nested shells
            bash("cd app && cat ~/.config/google-chrome/Default/Cookies"),
            bash("sudo cat ~/.config/google-chrome/Default/Cookies"),
            bash('bash -c "cat ~/.config/google-chrome/Default/Cookies"'),
            bash("echo $(cat ~/.config/google-chrome/Default/Cookies)"),
            bash("echo `cat ~/.config/google-chrome/Default/Cookies`"),
            bash("(cat ~/.config/google-chrome/Default/Cookies)"),
            bash('echo "$(cat ~/.config/google-chrome/Default/Cookies)"'),
            bash("rtk proxy cat ~/.config/google-chrome/Default/Cookies"),
            # relative forms: from component 0, and after a tracked cd
            bash("ls Library/Application\\ Support/Google"),
            bash("cd ~/Library/Application\\ Support && ls Google"),
            bash("cd ~/Library/Application\\ Support; cat Google/Chrome/Default/Cookies"),
            bash("cd ~/Library && cd Application\\ Support && ls G*"),
            bash("cd ~/.config && ls google-chrome"),
            bash("cd $HOME && ls .config/chromium"),
            bash("pushd ~/.config; ls chromium"),
            bash("cd ~/Library/Application\\ Support/Google/Chrome && cat Default/Cookies"),
            bash("cd ~/Library/Application\\ Support/Safari && ls ../Google"),
            bash("cd ~ && ls Library/Application\\ Support/Google"),
            # docker does not exempt G3
            bash("docker run -v " + HOME + "/.config/google-chrome:/p img"),
        ])

    def test_g3_profile_paths_in_file_tools(self):
        self.assert_table("G3", [
            ("Read", {"file_path": "$HOME/Library/Application Support/Google/Chrome/Default/Cookies"}),
            ("Read", {"file_path": HOME + "/Library/Application Support/Google/Chrome/Cookies"}),
            ("Read", {"file_path": "~/Library/Application Support/Google/Chrome/Cookies"}),
            ("Read", {"file_path": "${HOME}/.config/google-chrome/Default/Cookies"}),
            ("Grep", {"pattern": "session", "path": "~/.config/google-chrome"}),
            ("Grep", {"pattern": "x", "path": "~/library/application support/google/chrome"}),
            ("Grep", {"pattern": "x", "path": "~/Library/Application Support/Chromium"}),
            ("Glob", {"pattern": "**/Cookies", "path": "~/.config/chromium"}),
            ("Glob", {"pattern": "~/.config/google-chrome/**/Cookies"}),
            ("Glob", {"pattern": "Google/**", "path": "~/Library/Application Support"}),
            ("Glob", {"pattern": "G*/Chrome/*", "path": "~/Library/Application Support"}),
            ("Grep", {"pattern": "x", "path": "~/.config", "glob": "google-chrome/**"}),
            ("Edit", {"file_path": "~/.config/google-chrome/Default/Preferences",
                      "old_string": "a", "new_string": "b"}),
            ("Write", {"file_path": "~/.config/chromium/x", "content": "hi"}),
            ("NotebookEdit", {"notebook_path": "~/.config/chromium/n.ipynb", "new_source": "x"}),
            ("Read", {"file_path": OTHER_HOME + "/.config/google-chrome/Default/Cookies"}),
            # partly literal globs and wildcard-only windows under home (review N2)
            ("Glob", {"pattern": ".c*/c*/Default/Cookies", "path": HOME}),
            ("Glob", {"pattern": "L*/A*/G*/**/Cookies", "path": HOME}),
            ("Glob", {"pattern": HOME + "/.c*/c*/Default/Cookies"}),
            ("Read", {"file_path": HOME + "/.c*/c*/Default/Cookies"}),
            ("Grep", {"pattern": "x", "path": HOME, "glob": ".c*/chrom*/**"}),
            ("Glob", {"pattern": "**/*", "path": HOME}),
            ("Glob", {"pattern": "*/*/*", "path": "~"}),
            # nested values are visited too
            ("SomeTool", {"outer": {"paths": ["a", "~/.config/google-chrome/Default"]}}),
            ("SomeTool", {"params": [{"file_path": "~/.config/google-chrome/Default"}]}),
            ("SomeTool", {"source": "~/.config/chromium"}),
            ("SomeTool", {"paths": ["x", "~/.config/chromium"]}),
            ("SomeTool", {"target_dir": "~/Library/Application Support/Google"}),
            ("SomeTool", {"cwd": "~/.config/google-chrome"}),
            ("SomeTool", {"uri": "file://" + HOME + "/.config/chromium/Default"}),
            ("LS", {"path": "~/.config/chromium"}),
            ("NotebookRead", {"notebook_path": "~/.config/chromium/n.ipynb"}),
            ("mcp__filesystem__read_file", {"path": "~/.config/google-chrome/Default/Cookies"}),
            ("mcp__filesystem__read_multiple_files",
             {"paths": ["a.txt", "~/.config/google-chrome/Default/Cookies"]}),
        ])

    def test_g4_agent_browser(self):
        self.assert_table("G4", [
            bash("agent-browser open http://localhost:3000"),
            bash("agent-browser --session qa1 snapshot"),
            bash("npx agent-browser open x"),
            bash("npm i -g agent-browser"),
            bash("brew install agent-browser"),
            bash("agent-browser install"),
            bash("docker run --rm img agent-browser open x"),
            bash("/usr/local/bin/agent-browser open x"),
            bash("npx agent-browser@latest open x"),
            bash("npx -y agent-browser open x"),
            bash("pnpm dlx agent-browser open x"),
            bash("pnpm add agent-browser"),
            bash("npm install -g agent-browser@1.2.3"),
            bash("bunx agent-browser open x"),
            bash("docker run img:tag npx agent-browser open x"),
            bash("docker run --rm agent-browser:latest"),
            bash('docker run img sh -c "agent-browser open x"'),
            bash("docker exec c bash -lc 'agent-browser open x'"),
            bash("echo hi | agent-browser open x"),
            bash("cd x && agent-browser open y"),
            bash("npm uninstall x && agent-browser open y"),
            bash("find . -name '*.json' -exec agent-browser open {} \\;"),
        ])

    def test_g5_browser_mcp_tools(self):
        self.assert_table("G5", [
            ("mcp__playwright__browser_navigate", {"url": "https://x.dev"}),
            ("mcp__chrome-devtools__click", {"uid": "1"}),
            ("mcp__claude-in-chrome__navigate", {"url": "https://x.dev"}),
            ("mcp__puppeteer__puppeteer_navigate", {"url": "https://x.dev"}),
            ("mcp__plugin_playwright_playwright__browser_snapshot", {}),
            ("mcp__plugin_chrome-devtools_chrome-devtools__take_screenshot", {}),
            ("mcp__plugin_x_claude-in-chrome__tabs", {}),
            ("mcp__Playwright__browser_click", {}),
            ("mcp__chrome_devtools__click", {}),
            ("mcp__claude_in_chrome__navigate", {}),
        ])

    def test_g6_secrets_into_memory_in_file_tools(self):
        self.assert_table("G6", [
            ("Write", {"file_path": ".claude/memory/project-deploy.md",
                       "content": f"token: {GH_TOKEN}\u2026"}),
            ("Edit", {"file_path": ".claude/agent-memory/x/MEMORY.md", "old_string": "a",
                      "new_string": PRIVATE_KEY + "\nabc"}),
            ("Write", {"file_path": HOME + "/proj/.claude/memory/x.md", "content": AWS_KEY}),
            ("Write", {"file_path": "/work/.claude/agent-memory/a/b.md", "content": GH_PAT}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "key " + SK_KEY}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "x " + SLACK}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "Password = hunter2"}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "API_KEY: abc"}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "api-key=abc"}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "apikey: abc"}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "role " + "ASIA" + "ABCDEFGHIJKLMNOP"}),
            # a JSON- or dict-quoted key is an assignment too (FW-Q)
            ("Write", {"file_path": ".claude/memory/a.md", "content": '"' + PW + '": "' + PWV + '"'}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": "{'" + PW + "': '" + PWV + "'}"}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": '{"' + PW + '":"' + PWV + '"}'}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": '"api' + '_key": "' + PWV + '"'}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": '"apiKey" : ' + PWV}),
            ("Edit", {"file_path": ".claude/memory/a.md", "old_string": "a",
                      "new_string": '"' + PW.upper() + '"=' + PWV}),
            ("Write", {"file_path": ".Claude/Memory/a.md", "content": GH_TOKEN}),
            ("MultiEdit", {"file_path": ".claude/memory/a.md", "edits": [
                {"old_string": "x", "new_string": "fine"},
                {"old_string": "y", "new_string": GH_TOKEN}]}),
            ("Write", {"file_path": "docs/../.claude/memory/a.md", "content": GH_TOKEN}),
        ])

    def test_g6_secrets_into_memory_in_bash(self):
        mem = ".claude/memory/reference-aws.md"
        self.assert_table("G6", [
            bash(f'echo "{AWS_KEY}" >> {mem}'),
            bash("printf 'password: x' | tee .claude/agent-memory/r/n.md"),
            bash(f"echo {GH_TOKEN} > {mem}"),
            bash(f"echo {GH_TOKEN} >| {mem}"),
            bash(f"echo {GH_TOKEN} &> {mem}"),
            bash(f"echo {GH_TOKEN} &>> {mem}"),
            bash(f"echo {GH_TOKEN} 2> {mem}"),
            bash(f"echo {GH_TOKEN}>{mem}"),
            bash(f"echo {GH_TOKEN} | tee -a {mem}"),
            bash(f"cat > {mem} <<EOF\napi_key: abc\nEOF"),
            bash(f"cat >> {mem} <<'EOF'\n{PRIVATE_KEY}\nEOF"),
            bash(f"echo {GH_TOKEN} > {HOME}/p/.claude/agent-memory/a/b.md"),
            bash(f"echo {GH_TOKEN} > ~/p/.claude/memory/b.md"),
            bash(f"echo {GH_TOKEN} > $HOME/p/.claude/memory/b.md"),
            bash(f"cd .claude/memory && echo {GH_TOKEN} > a.md"),
            bash(f"echo {GH_TOKEN} | cp /dev/stdin {mem}"),
            bash(f"echo {GH_TOKEN} > /tmp/x && mv /tmp/x {mem}"),
            bash(f"echo {GH_TOKEN} > /tmp/x; install -m 600 /tmp/x {mem}"),
            bash(f"echo {GH_TOKEN} > /tmp/x; rsync /tmp/x .claude/memory/"),
            bash(f"ln -s /tmp/{GH_TOKEN} {mem}"),
            bash(f"echo {GH_TOKEN} | dd of={mem}"),
            bash(f"sed -i 's/a/{GH_TOKEN}/' {mem}"),
            bash(f"perl -pi -e 's/a/{GH_TOKEN}/' {mem}"),
            bash(f"bash -c 'echo {GH_TOKEN} >> {mem}'"),
            bash(f"echo password=x | sudo tee {mem}"),
        ])

    def test_chains_wrappers_and_prefixes(self):
        self.assert_table("G1", [
            bash("cd app && npx playwright install"),
            bash("CI=1 npx playwright install"),
            bash('bash -c "npx playwright install"'),
            bash("sudo npx playwright install"),
            bash("docker ps; npx playwright install"),
            bash("rtk npx playwright install"),
            # more separators and shells
            bash("ls || npx playwright install"),
            bash("ls | npx playwright install"),
            bash("ls & npx playwright install"),
            bash("ls\nnpx playwright install"),
            bash("ls &&\\\n npx playwright install"),
            bash("sh -c 'npx playwright install'"),
            bash("zsh -c 'npx playwright install'"),
            bash("dash -c 'npx playwright install'"),
            bash("ksh -c 'npx playwright install'"),
            bash("/bin/bash -lc 'npx playwright install'"),
            bash("bash -ec 'npx playwright install'"),
            bash("bash --norc -c 'npx playwright install'"),
            bash("sudo bash -c 'cd x && npx playwright install'"),
            bash("bash -c \"bash -c 'npx playwright install'\""),
            bash("sudo -u root npx playwright install"),
            bash("sudo -E -H npx playwright install"),
            bash("doas npx playwright install"),
            bash("env npx playwright install"),
            bash("env -i FOO=1 npx playwright install"),
            bash("env -u HOME npx playwright install"),
            bash("env -S 'npx playwright install'"),
            bash("command npx playwright install"),
            bash("exec npx playwright install"),
            bash("exec -a x npx playwright install"),
            bash("time npx playwright install"),
            bash("time -p npx playwright install"),
            bash("nohup npx playwright install &"),
            bash("nice -n 10 npx playwright install"),
            bash("timeout 60 npx playwright install"),
            bash("timeout -s KILL 60 npx playwright install"),
            bash("xargs npx playwright install"),
            bash("echo x | xargs -n1 npx playwright install"),
            bash("xvfb-run -a npx playwright test"),
            bash("caffeinate -i npx playwright install"),
            bash("stdbuf -oL npx playwright install"),
            bash("watch -n 5 npx playwright install"),
            bash("watch 'npx playwright install'"),
            bash("! npx playwright install"),
            bash("if true; then npx playwright install; fi"),
            bash("for i in 1 2; do npx playwright install; done"),
            bash("while true; do npx playwright install; done"),
            bash("{ npx playwright install; }"),
            bash("(npx playwright install)"),
            bash("( cd x && npx playwright install )"),
            bash("echo $(npx playwright install)"),
            bash("echo `npx playwright install`"),
            bash('echo "$(npx playwright install)"'),
            bash('echo "`npx playwright install`"'),
            bash("echo $(echo $(npx playwright install))"),
            bash("FOO=1 BAR=2 sudo env X=3 rtk npx playwright install"),
            bash("rtk -u npx playwright install"),
            bash("rtk bypass npx playwright install"),
            bash("rtk proxy npx playwright install"),
            bash("sudo rtk proxy npx playwright install"),
            bash("npx playwright install 2>&1 | tail -5"),
            bash("npx playwright install > /dev/null 2>&1"),
            bash("npx playwright install # for tests"),
            bash("# header\nnpx playwright install"),
            bash("npx playwright install\\\n  chromium"),
            # package runner forms
            bash("pnpm dlx playwright install"),
            bash("npm exec playwright install"),
            bash("npm exec -- playwright install"),
            bash("npm exec --yes playwright install"),
            bash("yarn dlx playwright install"),
            bash("yarn exec playwright install"),
            bash("yarn playwright install"),
            bash("pnpm playwright install"),
            bash("bun x playwright install"),
            bash("bunx playwright install"),
            bash("pnpx playwright install"),
            bash("npx --yes playwright install"),
            bash("npx -p playwright playwright install"),
            bash("npx --package=playwright playwright install"),
            bash("npx -c 'playwright install'"),
            bash("npm exec -c 'playwright install'"),
            bash("uvx playwright install"),
            bash("pipx run playwright install"),
            bash("pnpm --filter web exec playwright install"),
            bash("pnpm -C web dlx playwright install"),
            bash("npm --prefix web exec playwright install"),
            bash("yarn --cwd web playwright install"),
            bash("pnpm -C web add puppeteer"),
            bash("npm --prefix web install puppeteer"),
            # Python and JS project runners (review I3)
            bash("uv run playwright install chromium"),
            bash("uv run python -m playwright install"),
            bash("uv run --with playwright playwright install"),
            bash("uv tool run playwright install"),
            # value flags and global flags around `run` (round 2)
            bash("uv run --project e2e npx playwright install"),
            bash("uv run --directory e2e playwright install"),
            bash("uv run --env-file .env playwright install"),
            bash("uv run --extra dev playwright install"),
            bash("uv run --group test playwright install"),
            bash("uv --directory e2e run playwright install"),
            bash("uv --project e2e run --extra dev npx playwright install"),
            bash("poetry -C sub run playwright install"),
            bash("poetry run -P sub playwright install"),
            bash("hatch -e test run playwright install"),
            bash("pdm -p sub run playwright install"),
            bash("pipenv --python 3.12 run playwright install"),
            bash("poetry run playwright install"),
            bash("pipenv run playwright install"),
            bash("pdm run playwright install"),
            bash("hatch run playwright install"),
            bash("rye run playwright install"),
            bash("conda run -n e2e playwright install"),
            bash("conda run --prefix /opt/e2e playwright install"),
            bash("yarn run playwright install"),
            bash("bun run playwright install"),
            bash("pnpm run playwright install"),
            bash("pnpm --filter web run playwright install"),
            bash("pnpm --filter web playwright install"),
            bash("yarn workspace e2e playwright install"),
            bash("yarn workspace e2e run playwright install"),
            # the Playwright scaffolder downloads browsers (review I4, R21)
            bash("npm init playwright@latest"),
            bash("npm init playwright@latest -- --quiet --browser=chromium"),
            bash("yarn create playwright"),
            bash("pnpm create playwright"),
            bash("bun create playwright"),
            bash("npx create-playwright@latest"),
            bash("create-playwright"),
            bash("npm --prefix web init playwright"),
            # reinstall / upgrade of a browser package (review M2)
            bash("brew reinstall --cask google-chrome"),
            bash("brew upgrade chromium"),
            # wrapper flag forms (review M1)
            bash("sudo -iu bob npx playwright install"),
            bash("sudo --user bob npx playwright install"),
            bash("rtk proxy -- npx playwright install"),
            bash("xargs --max-args 1 npx playwright install"),
            bash("coproc npx playwright install"),
            # a substitution whose heredoc is closed is still analyzed (review I1)
            bash("echo \"$(cat <<'EOF'\nx\nEOF\n)\"; npx playwright install"),
            bash("git commit -m \"$(cat <<'EOF'\nit's fine\nEOF\n)\" && npx playwright install"),
            bash("echo $(npx playwright install)"),
            bash("x=\"$(\nnpx playwright install\n)\""),
            # an unquoted heredoc body runs its substitutions (review N4a)
            bash("git commit -F - <<EOF\nuse `npx playwright install` now\nEOF"),
            bash("cat <<EOF\n$(npx playwright install)\nEOF"),
            bash("git commit -m \"$(cat <<EOF\nit's `npx playwright install`\nEOF\n)\""),
            bash("cat <<-EOF\n\t$(npx playwright install)\n\tEOF"),
            bash("cat <<EOF\nline 1\nsafe $(echo $(npx playwright install)) tail\nEOF"),
            bash("git commit -m \"$(cat <<EOF\nok\n$(npx playwright install)\nEOF\n)\""),
            # array literals: substitutions inside them run (review N1)
            bash("arr=($(npx playwright install))"),
            bash("arr=(`npx playwright install`)"),
            bash("arr=(\"$(npx playwright install)\")"),
            bash("arr+=($(npx playwright install))"),
            bash("declare -a arr=($(npx playwright install))"),
            bash("arr=(a b\n  $(npx playwright install)\n)"),
            # multi-line arrays with comments (round 3)
            bash("arr=(\n  x # c\n  $(npx playwright install)\n)"),
            bash("arr=( # it's a list\n  $(npx playwright install)\n)"),
            bash("arr=(\n  \"$(cat <<'EOF'\nsay \"hi\" it's\nEOF\n)\"\n  $(npx playwright install)\n)"),
            # a backslash-newline before a heredoc does not make its delimiter look quoted
            bash("cat \\\n<<EOF\n$(npx playwright install)\nEOF"),
            bash("git commit -F - \\\n  <<EOF\nuse `npx playwright install`\nEOF"),
            bash("cat 2\\\n<<EOF\n$(npx playwright install)\nEOF"),
            # nesting up to the depth cap (review M9)
            bash("echo $(echo $(echo $(echo $(echo $(echo $(echo $(npx playwright install)))))))"),
            # macOS / coreutils wrappers, quoting tricks and substituted program names
            bash("arch -arm64 npx playwright install"),
            bash("gtimeout 60 npx playwright install"),
            bash("setsid npx playwright install"),
            bash("ionice -c 3 npx playwright install"),
            bash("unbuffer npx playwright install"),
            bash("$(which npx) playwright install"),
            bash("`which npx` playwright install"),
            bash("\\npx playwright install"),
            bash('"npx" playwright "install"'),
            bash("n'p'x playwright install"),
            # absolute path and find -exec
            bash("/usr/local/bin/npx playwright install"),
            bash("find . -name x -exec npx playwright install \\;"),
            # literal text piped into a shell
            bash('echo "npx playwright install" | sh'),
            bash("echo 'npx playwright install' | bash"),
            bash("printf 'ls\\nnpx playwright install\\n' | bash"),
            bash("echo 'npx playwright install' | sudo bash -s"),
            bash("bash <<EOF\nnpx playwright install\nEOF"),
            bash("sh <<'EOF'\nls\nnpx playwright install\nEOF"),
            bash("cat <<EOF | sh\nnpx playwright install\nEOF"),
            bash("sh <<-EOF\n\tnpx playwright install\n\tEOF"),
            bash("bash <<< 'npx playwright install'"),
            bash("eval 'npx playwright install'"),
            bash("eval npx playwright install"),
            # heredoc with apostrophes before the command
            bash("cat <<EOF\nit's fine\nEOF\nnpx playwright install"),
        ])
        self.assert_table("G2", [
            bash('rtk bypass open -a "Google Chrome"'),
            bash('sudo open -a "Google Chrome"'),
            bash('cd x && open -a "Google Chrome"'),
            bash('FOO=1 open -a "Google Chrome"'),
            bash('bash -c \'open -a "Google Chrome"\''),
            bash('nohup chromium --headless &'),
            bash('env DISPLAY=:0 google-chrome'),
            bash('xvfb-run -a chromium'),
        ])
        self.assert_table("G4", [
            bash("cd x && agent-browser open y"),
            bash("rtk proxy agent-browser open x"),
            bash("sudo agent-browser open x"),
            bash("FOO=1 agent-browser open x"),
            bash("bash -c 'agent-browser open x'"),
            bash("nohup agent-browser open x &"),
            bash("timeout 10 agent-browser open x"),
            bash("echo hi; agent-browser open x"),
            bash("echo $(agent-browser open x)"),
        ])

    # ---------------------------------------------------------------- allowed

    def test_allowed_commands(self):
        self.assert_table(None, [
            bash("grep -r playwright package.json"),
            bash("grep -r agent-browser ."),
            bash('git commit -m "drop puppeteer and playwright install"'),
            bash("open -a BrowserOS"),
            bash("npm install"),
            bash("npm i puppeteer-core"),
            bash("docker run --rm mcr.microsoft.com/playwright:v1.49.0 npx playwright test"),
            bash("docker compose run e2e npx playwright test"),
            bash("rtk git status"),
            bash("rtk bypass git log"),
            ("Read", {"file_path": "src/chrome-extension/manifest.json"}),
            ("mcp__browseros-neo__navigate", {"url": "https://x.dev"}),
            ("WebFetch", {"url": "https://x.dev", "prompt": "summarize"}),
            ("Write", {"file_path": ".claude/memory/feedback-tests.md",
                       "content": "Covers the password reset flow"}),
            ("Write", {"file_path": "src/config.ts", "content": "const t = '" + GH_TOKEN + "'"}),
        ])

    def test_allowed_g1_neighbours(self):
        self.assert_table(None, [
            bash("npm i playwright"),
            bash("npm install playwright"),
            bash("npm i puppeteer-core"),
            bash("npm install @puppeteer/browsers"),
            bash("npm i -D vitest"),
            bash("pnpm add puppeteer-core"),
            bash("pnpm install"),
            bash("yarn"),
            bash("yarn install"),
            bash("yarn add react"),
            bash("npm uninstall puppeteer"),
            bash("npx playwright --version"),
            bash("npx playwright"),
            bash("npx playwright show-report"),
            bash("npx vitest"),
            bash("npx tsc --noEmit"),
            bash("pip install requests"),
            bash("pip install pytest-playwright"),
            bash("pip install -r requirements.txt"),
            bash("pip uninstall playwright"),
            bash("pip list | grep playwright"),
            bash("pip freeze"),
            bash("python -m pytest tests"),
            bash("python -m venv .venv"),
            bash("python3 script.py"),
            bash("brew install jq"),
            bash("brew install --cask orbstack"),
            bash("brew install browseros"),
            bash("brew search chromium"),
            bash("brew uninstall chromium"),
            bash("brew list"),
            bash("apt-get install -y curl"),
            bash("apt-get update"),
            bash("pacman -Ss chromium"),
            bash("snap list"),
            bash("echo playwright install"),
            bash("echo 'npx playwright install'"),
            bash("cat docs/playwright.md"),
            bash("cat <<EOF > notes.md\nrun npx playwright install later\nEOF"),
            bash("git commit -m 'docs: why npx playwright install is blocked'"),
            bash("git log --grep='npx playwright install'"),
            bash("gh pr create --title 'remove puppeteer install' --body 'x'"),
            bash("which playwright"),
            bash("ls node_modules/playwright"),
            bash("rm -rf node_modules/.cache/puppeteer"),
            bash("grep -rn 'playwright install' ."),
            bash("sed -n '1,5p' playwright.config.ts"),
            bash("npm run test:e2e"),
        ])

    def test_allowed_commit_and_pr_heredoc_messages(self):
        """`"$(cat <<'EOF' ... EOF)"` bodies are text, however many quotes or `)` they hold (I1)."""
        self.assert_table(None, [
            bash("git commit -m \"$(cat <<'EOF'\nfeat: drop agent-browser\n\nIt's replaced by BrowserOS Neo.\nEOF\n)\""),
            bash("git commit -m \"$(cat <<'EOF'\nchore: e2e\n\nplaywright test now runs in docker; it's faster.\nEOF\n)\""),
            bash("gh pr create --title x --body \"$(cat <<'EOF'\n## Summary\n- Removes agent-browser; we don't need it\nEOF\n)\""),
            bash("git commit -m \"$(cat <<'EOF'\nfix: it's done\n\nchromium is never launched\nEOF\n)\""),
            bash("git commit -m \"$(cat <<'EOF'\nfix: it's done\n\n~/.config/chromium is now protected\nEOF\n)\""),
            bash("git commit -m \"$(cat <<'EOF'\nfix: done\nagent-browser :)\nEOF\n)\""),
            # same forms without apostrophes, with several, unquoted or dashed delimiters
            bash("git commit -m \"$(cat <<'EOF'\nfix: done\nnpx playwright install (1)\nEOF\n)\""),
            bash("git commit -m \"$(cat <<EOF\nit's 'quoted' \"too\" (and) agent-browser\nEOF\n)\""),
            bash("git commit -m \"$(cat <<-EOF\n\tagent-browser it's\n\tEOF\n)\""),
            bash("git commit -m \"$(cat <<\"EOF\"\nchromium :) it's\nEOF\n)\" && git push"),
            bash("x=$(cat <<'EOF'\nagent-browser it's :)\nEOF\n); echo $x"),
            bash("cat <<'EOF' | git commit -F -\nfix: it's agent-browser :)\nEOF"),
            bash("git commit -m \"$(cat <<'EOF'\nline\nEOF\n)\"\ngit status"),
        ])

    def test_allowed_wildcard_only_paths(self):
        """Wildcard-only components name no particular path (I2)."""
        self.assert_table(None, [
            ("Glob", {"pattern": "**/*"}),
            ("Glob", {"pattern": "**/*", "path": HOME + "/proj"}),
            ("Glob", {"pattern": "src/**/*", "path": HOME + "/proj"}),
            ("Glob", {"pattern": "**/*.py", "path": "src"}),
            ("Glob", {"pattern": "*/*/*"}),
            ("Grep", {"pattern": "TODO", "glob": "**/*"}),
            ("Grep", {"pattern": "TODO", "path": HOME + "/proj", "glob": "*/*"}),
            bash("ls */*"),
            bash("ls -d */*/"),
            bash("git add */*"),
            bash("ls */*/*"),
            bash("du -sh " + HOME + "/proj/*/*"),
            bash("ls ?/?/?"),
            bash("cd ~/proj && ls */*"),
            bash("ls src/*/*"),
            bash("ls ./*/*/*"),
            bash("ls ../*/*"),
            bash("cd " + HOME + "/proj/sub && ls */*/*"),
            bash("ls ~/proj/*/*"),
            bash("ls ~/*"),
            bash("ls ~/*.md"),
            ("Glob", {"pattern": "**/*", "path": "~/proj"}),
            ("Glob", {"pattern": "*/*/*", "path": HOME + "/proj/src"}),
        ])

    def test_allowed_free_text_fields_are_not_paths(self):
        """Prompts, queries and todos may mention a profile root (R22)."""
        self.assert_table(None, [
            ("Agent", {"prompt": "~/.config/chromium must never be read; summarize docs"}),
            ("Agent", {"prompt": "/home/tester/.config/google-chrome is off-limits",
                       "description": "~/.config/chromium", "subagent_type": "general-purpose"}),
            ("TodoWrite", {"todos": [{"content": "audit ~/.config/chromium", "status": "pending",
                                      "activeForm": "~/.config/chromium audit"}]}),
            ("WebSearch", {"query": "~/.config/chromium cookies"}),
            ("Bash", {"command": "ls", "description": "~/.config/chromium"}),
            ("SomeTool", {"message": "~/Library/Application Support/Google"}),
            ("SomeTool", {"nested": {"text": ["~/.config/google-chrome"]}}),
        ])

    def test_allowed_round_1_neighbours(self):
        self.assert_table(None, [
            # runners and scaffolders that are not browser installs
            bash("uv run pytest"),
            bash("uv run python -m pytest"),
            bash("uv run --with rich python app.py"),
            bash("uv tool run ruff check ."),
            bash("uv pip install requests"),
            bash("poetry run pytest"),
            bash("pipenv run python app.py"),
            bash("conda run -n e2e python app.py"),
            bash("npm run build"),
            bash("npm run playwright"),
            bash("yarn run build"),
            bash("bun run dev"),
            bash("pnpm run lint"),
            bash("pnpm --filter web run build"),
            bash("yarn workspace web build"),
            bash("npm init -y"),
            bash("npm init vite"),
            bash("npm create vite@latest"),
            bash("yarn create react-app x"),
            bash("npx create-react-app x"),
            bash("npm init playwright@latest -- --no-browsers"),
            bash("npx create-playwright@latest --no-browsers"),
            bash("npm init playwright -- --quiet --no-browsers"),
            # inspection of agent-browser / browsers
            bash("npm ls agent-browser"),
            bash("npm view agent-browser version"),
            bash("pnpm why agent-browser"),
            bash("yarn list --pattern agent-browser"),
            bash("npm outdated agent-browser"),
            bash("brew info agent-browser"),
            bash("brew list agent-browser"),
            bash("brew search agent-browser"),
            bash("brew list"),
            bash("brew upgrade jq"),
            bash("brew reinstall jq"),
            bash("echo --remote-debugging-port"),
            bash("printf '%s\\n' --remote-debugging-port=9222"),
            bash("browsers=(chromium firefox); echo ${browsers[@]}"),
            bash("names+=(chromium) && echo done"),
            # array elements are data, not a command (known limit R25: a command built in a
            # variable, `"${cmd[@]}"`, is not resolved)
            bash("tools=(agent-browser); echo ${#tools[@]}"),
            bash("cmd=(chromium --remote-debugging-port=9222); echo ${cmd[@]}"),
            bash("tools=(agent-browser); \"${tools[0]}\" open x"),
            bash("cmd=(chromium --remote-debugging-port=9222); \"${cmd[@]}\""),
            bash("files=(a.txt b.txt)\nls \"${files[@]}\""),
            bash("files=(\n  a.txt\n  b.txt\n)"),
            bash("arr=($(ls) \"$(date)\" `pwd`)"),
            bash("declare -a f=(src/*.py)"),
            bash("files=(src/*.py)"),
            bash("args=(--flag \"$x\")"),
            bash("names=(chromium firefox webkit)"),
            bash("pids=($(pgrep -f chromium))"),
            bash("names=(\n  chromium # it's here\n  firefox\n  # it's a comment\n  webkit\n)"),
            bash("arr=(\n  \"$(cat <<'EOF'\nnpx playwright install\nagent-browser it's\nEOF\n)\"\n  b\n)"),
            bash("cat \\\n<<'EOF'\n$(npx playwright install)\nEOF"),
            # unquoted heredoc bodies without a blocked command, and quoted ones stay data
            bash("cat <<EOF\n$(date) and `pwd`\nEOF"),
            bash("git commit -F - <<'EOF'\nuse `npx playwright install` now\nEOF"),
            bash("cat <<'EOF'\n$(npx playwright install)\nEOF"),
            bash("git commit -m \"$(cat <<'EOF'\nit's `npx playwright install`\nEOF\n)\""),
            bash("cat <<\"EOF\"\n$(npx playwright install)\nEOF"),
            bash("cat <<\\EOF\n$(npx playwright install)\nEOF"),
            bash("cat <<EOF\n\\$(npx playwright install)\nEOF"),            bash("sudo -iu bob ls"),
            bash("rtk proxy -- git status"),
            bash("xargs --max-args 1 echo"),
            bash("tar czf /tmp/p.tgz -C src docs"),
            bash("git -C ~/Library/Application\\ Support/Code status"),
            bash("make -C src all"),
            bash("tar czf /tmp/p.tgz -C ~/.config git"),
            bash("coproc ls"),
            # a container may run what the host may not, via heredoc too
            bash("docker run -i img sh <<EOF\nnpx playwright install\nEOF"),
            bash("docker exec -i c bash -s <<'EOF'\nchromium --headless\nEOF"),
            bash("echo 'npx playwright install' | docker run -i img sh"),
            bash("docker run -i img cat <<EOF\nagent-browser open x\nEOF"),
        ])

    def test_allowed_docker_exemption(self):
        self.assert_table(None, [
            bash("docker run --rm mcr.microsoft.com/playwright:v1.49.0 npx playwright test"),
            bash("docker compose run e2e npx playwright test"),
            bash("docker-compose run e2e npx playwright install"),
            bash("docker exec e2e npx playwright install"),
            bash("sudo docker run img npx playwright install"),
            bash("rtk docker run img npx playwright install"),
            bash("docker run img sh -c 'npx playwright install && npx playwright test'"),
            bash("docker run img bash -lc 'chromium --headless --remote-debugging-port=9222'"),
            bash("docker run img chromium --remote-debugging-port=9222"),
            bash("docker run --rm img google-chrome --headless"),
            bash("docker run --rm img pip install playwright"),
            bash("docker build -t e2e . && docker run e2e npx playwright test"),
            bash("cd e2e && docker compose up --build"),
            bash("docker run -v $PWD:/w -w /w img sh -c 'pip install playwright'"),
        ])

    def test_docker_exempts_only_g1_and_g2(self):
        self.assert_table("G4", [
            bash("docker run img sh -c 'agent-browser open x'"),
            bash("docker run img agent-browser open x"),
            bash("docker compose run e2e npx agent-browser open x"),
            # a heredoc fed to a shell inside a container keeps G4 (review M5)
            bash("docker run -i img sh <<EOF\nagent-browser open x\nEOF"),
            bash("docker exec -i c bash -s <<'EOF'\nls\nagent-browser open x\nEOF"),
            bash("echo 'agent-browser open x' | docker run -i img sh"),
        ])
        self.assert_table("G3", [
            bash("docker run -v ~/.config/google-chrome:/p img npx playwright test"),
            bash("docker run img sh -c 'cat ~/.config/google-chrome/Default/Cookies'"),
            bash("docker run img sh -c 'cd ~/.config && ls chromium'"),
        ])
        # Host commands chained with a docker command are not exempt.
        self.assert_table("G1", [
            bash("docker ps && npx playwright install"),
            bash("docker run img true; npx playwright install"),
            bash("docker run img true | npx playwright install"),
            bash("echo $(npx playwright install) | docker run -i img cat"),
            bash("docker run img echo $(npx playwright install)"),
        ])

    def test_allowed_g2_neighbours(self):
        self.assert_table(None, [
            bash("open -a BrowserOS"),
            bash('open -a "BrowserOS Neo" https://x.dev'),
            bash("open https://example.com"),
            bash("open ."),
            bash("open -a Safari https://chrome.google.com"),
            bash("open -a Terminal"),
            bash("open -b com.apple.Safari"),
            bash("open Notes.app"),
            bash("open src/chrome-extension/"),
            bash("which google-chrome"),
            bash("command -v chromium"),
            bash("ls /Applications/Google\\ Chrome.app"),
            bash("grep -r remote-debugging-port ."),
            bash("grep -r -- --remote-debugging-port docs"),
            bash("rg --remote-debugging-port src"),
            bash("git log --grep=--remote-debugging-port"),
            bash("git commit -m 'chromium --remote-debugging-port=9222 is blocked'"),
            bash("echo chromium"),
            bash("cat chrome-notes.md"),
            bash("pgrep -f chromium"),
            bash("osascript -e 'tell application \"Finder\" to activate'"),
            bash("ls chrome-extension"),
            bash("node chrome-extension/build.js"),
            bash("npm run build:chrome"),
            bash("npm install chromium-bidi"),
        ])

    def test_allowed_g3_neighbours(self):
        self.assert_table(None, [
            bash("ls ~/Library"),
            bash("ls ~/Library/Application\\ Support"),
            bash("ls ~/Library/Application\\ Support/Code"),
            bash("ls ~/Library/Application\\ Support/BrowserOS"),
            bash("ls ~/.config"),
            bash("ls ~/.config/git"),
            bash("cat ~/.config/gh/hosts.yml"),
            bash("ls ~/Library/Caches"),
            bash("ls ~/Library/Preferences"),
            bash("ls ~"),
            bash("cat ~/.zshrc"),
            bash("ls src/google-chrome-notes"),
            bash("ls docs/chromium"),
            bash("cat docs/application-support/google.md"),
            bash("cat ~/Documents/Google/notes.txt"),
            bash("ls ~/Google"),
            bash("git commit -m 'never read ~/Library/Application Support/Google'"),
            bash("git commit -m 'docs: ~/.config/google-chrome is off-limits'"),
            bash("echo 'profiles live in ~/.config/google-chrome'"),
            bash("grep -rn 'google-chrome' docs"),
            bash("cd ~/Library && ls"),
            bash("cd ~/Library/Application\\ Support && ls Code"),
            bash("cd ~/.config && ls git"),
            bash("cd ~/Library/Application\\ Support/Code && ls ../Notes"),
            bash("ls ~/Library/Application\\ Support/{Code,Notes}"),
            bash("echo ${HOME}/Library/Caches"),
            ("Read", {"file_path": "src/chrome-extension/manifest.json"}),
            ("Read", {"file_path": "docs/google-chrome.md"}),
            ("Read", {"file_path": "~/Library/Application Support/Code/User/settings.json"}),
            ("Grep", {"pattern": "chromium", "path": "src"}),
            ("Grep", {"pattern": ".config/google-chrome", "path": "docs"}),
            ("Grep", {"pattern": "Library/Application Support/Google", "path": "docs", "glob": "*.md"}),
            ("Glob", {"pattern": "**/*.md", "path": "docs"}),
            ("Glob", {"pattern": "*", "path": "~/Library"}),
            ("Write", {"file_path": "notes.md",
                       "content": "Chrome profiles live in ~/Library/Application Support/Google"}),
            ("Edit", {"file_path": "README.md", "old_string": "~/.config/google-chrome",
                      "new_string": "~/.config/chromium"}),
            ("Agent", {"prompt": "Never touch ~/Library/Application Support/Google or ~/.config/chromium"}),
        ])

    def test_allowed_g4_neighbours(self):
        self.assert_table(None, [
            bash("grep -r agent-browser ."),
            bash("egrep agent-browser README.md"),
            bash("fgrep -r agent-browser ."),
            bash("rg agent-browser"),
            bash("ag agent-browser"),
            bash("ack agent-browser"),
            bash("rtk grep -rn agent-browser src"),
            bash("git grep agent-browser"),
            bash("git log --oneline -- docs/agent-browser.md"),
            bash("git add docs/agent-browser"),
            bash("echo agent-browser"),
            bash("printf '%s\\n' agent-browser"),
            bash("which agent-browser"),
            bash("type agent-browser"),
            bash("command -v agent-browser"),
            bash("ls vendor/agent-browser"),
            bash("cat docs/agent-browser"),
            bash("head -n 5 docs/agent-browser"),
            bash("tail docs/agent-browser"),
            bash("less docs/agent-browser"),
            bash("wc -l docs/agent-browser"),
            bash("file docs/agent-browser"),
            bash("stat docs/agent-browser"),
            bash("rm -rf vendor/agent-browser"),
            bash("cd vendor/agent-browser"),
            bash("mkdir -p docs/agent-browser"),
            bash("touch docs/agent-browser"),
            bash("find . -name agent-browser -type d"),
            bash("cp -r tmpl docs/agent-browser"),
            bash("mv docs/agent-browser docs/browser-notes"),
            bash("diff docs/agent-browser docs/other"),
            bash("du -sh vendor/agent-browser"),
            bash("npm uninstall -g agent-browser"),
            bash("npm remove agent-browser"),
            bash("npm rm agent-browser"),
            bash("npm un agent-browser"),
            bash("npm r agent-browser"),
            bash("pnpm remove agent-browser"),
            bash("yarn remove agent-browser"),
            bash("bun remove agent-browser"),
            bash("brew uninstall agent-browser"),
            bash("brew remove agent-browser"),
            bash("brew rm agent-browser"),
            bash('git commit -m "remove agent-browser from the docs"'),
            bash("git commit -m 'agent-browser is not used here'"),
            bash("cat docs/agent-browser.md"),
            bash("curl -s https://github.com/vercel-labs/agent-browser"),
            bash("echo 'run agent-browser open x' > notes.md"),
            bash("cat <<EOF > notes.md\nagent-browser open x\nEOF"),
            bash("open -a BrowserOS"),
            bash("agent-browsers --help"),
            bash("my-agent-browser open x"),
            bash("agent_browser open x"),
            ("Read", {"file_path": "docs/agent-browser/README.md"}),
            ("Write", {"file_path": "docs/a.md", "content": "agent-browser open x"}),
        ])

    def test_allowed_g5_neighbours(self):
        self.assert_table(None, [
            ("mcp__browseros-neo__navigate", {}),
            ("mcp__plugin_anthology_browseros-neo__snapshot", {}),
            ("mcp__github__list_issues", {}),
            ("mcp__slack__post", {}),
            ("Playwright", {}),
            ("playwright", {}),
            ("browser_playwright", {}),
            ("Bash", {"command": "ls"}),
            ("WebFetch", {"url": "https://playwright.dev"}),
            ("WebSearch", {"query": "playwright vs puppeteer"}),
            ("Task", {"prompt": "use playwright"}),
            ("", {}),
        ])

    def test_allowed_g6_neighbours(self):
        mem = ".claude/memory/feedback-tests.md"
        self.assert_table(None, [
            # secret-looking text outside the memory folders
            ("Write", {"file_path": "src/config.ts", "content": GH_TOKEN}),
            ("Write", {"file_path": "docs/memory/a.md", "content": GH_TOKEN}),
            ("Write", {"file_path": ".claude/memory-notes/a.md", "content": GH_TOKEN}),
            ("Write", {"file_path": ".claude/settings.json", "content": "password: x"}),
            ("Write", {"file_path": ".claude/memory.md", "content": GH_TOKEN}),
            ("Write", {"file_path": "memory/.claude/a.md", "content": GH_TOKEN}),
            ("Write", {"file_path": ".claude/agent-memory", "content": "x"}),
            # clean memory writes
            ("Write", {"file_path": mem, "content": "Covers the password reset flow"}),
            ("Write", {"file_path": mem, "content": "ghp_ prefixes mark GitHub tokens"}),
            ("Write", {"file_path": mem, "content": "api key rotation happens monthly"}),
            ("Write", {"file_path": mem, "content": "the sk-learn library is used"}),
            ("Write", {"file_path": mem, "content": "AKIA prefix marks AWS keys"}),
            ("Write", {"file_path": mem, "content": "Use `password` fields carefully"}),
            # quoted keys with an empty, null or schema-shaped value are not assignments (FW-Q)
            ("Write", {"file_path": mem, "content": '"' + PW + '": {"type": "string"}'}),
            ("Write", {"file_path": mem, "content": '"' + PW + '": ""'}),
            ("Write", {"file_path": mem, "content": "{'" + PW + "': ''}"}),
            ("Write", {"file_path": mem, "content": '"api' + '_key": null'}),
            ("Write", {"file_path": mem, "content": '"' + PW + '": true, "api_key": []'}),
            ("Write", {"file_path": mem, "content": "the " + PW + " field"}),
            ("Write", {"file_path": mem, "content": 'the "' + PW + '" property is required'}),
            ("Write", {"file_path": mem, "content": "A PRIVATE KEY is never stored"}),
            ("Edit", {"file_path": mem, "old_string": GH_TOKEN, "new_string": "[redacted]"}),
            ("Read", {"file_path": mem}),
            ("MultiEdit", {"file_path": mem, "edits": [{"old_string": GH_TOKEN, "new_string": "x"}]}),
            # shell commands that do not write into memory
            bash(f"cat {mem}"),
            bash(f"grep -n password {mem}"),
            bash(f"grep -c {GH_TOKEN} {mem}"),
            bash(f"echo {GH_TOKEN} > /tmp/x"),
            bash(f"echo {GH_TOKEN} | cat"),
            bash(f"echo {GH_TOKEN} > src/config.ts"),
            bash("echo 'password reset flow' >> " + mem),
            bash("echo hello > .claude/memory/a.md"),
            bash("printf 'token notes' | tee .claude/agent-memory/r/n.md"),
            bash(f"cat {mem} > /tmp/copy"),
            bash(f"cp {mem} /tmp/copy"),
            bash(f"sed -n '1,3p' {mem}"),
            bash(f"git add {mem}"),
            bash(f"git commit -m 'save memory {GH_TOKEN}'"),
            bash(f"ls .claude/memory > /tmp/{GH_TOKEN}"),
        ])

    def test_allowed_prose_and_odd_but_ordinary_commands(self):
        self.assert_table(None, [
            bash(""),
            bash("   "),
            bash("ls"),
            bash("git status"),
            bash("echo it's fine"),
            bash("echo \"don't\" && ls"),
            bash("cat <<EOF\ndon't stop\nEOF"),
            bash("cat <<'EOF' > a.txt\nit's here\nEOF\nls"),
            bash("echo $((1<<3))"),
            bash("x=$((2<<4)); echo $x"),
            bash("for f in *.md; do echo \"$f\"; done"),
            bash("ls -la | grep -v '^d' | wc -l"),
            bash("find . -name '*.py' -exec wc -l {} +"),
            bash("find . -name '*.py' -exec grep -l playwright {} \\;"),
            bash("python3 -m unittest discover -s tests"),
            bash("npx prettier --check ."),
            bash("env"),
            bash("sudo ls"),
            bash("time ls"),
            bash("rtk"),
            bash("rtk git diff"),
            bash("bash -c 'ls'"),
            bash("bash script.sh"),
            bash("sh -c"),
            bash("watch ls"),
            bash("timeout 5 sleep 1"),
            bash("xargs echo"),
            bash("FOO=bar"),
            bash("cd"),
            bash("cd -"),
            bash("cd /tmp && ls && cd .."),
            ("Bash", {"command": "ls", "description": "list files"}),
            ("Read", {"file_path": "README.md"}),
            ("Edit", {"file_path": "a.py", "old_string": "x", "new_string": "y"}),
            ("Write", {"file_path": "a.py", "content": "print(1)"}),
            ("Grep", {"pattern": "playwright", "path": "docs"}),
            ("Glob", {"pattern": "**/*.py"}),
            ("TodoWrite", {"todos": [{"content": "install playwright", "status": "pending"}]}),
        ])

    def test_g3_strings_in_content_fields_are_not_paths(self):
        self.assert_table(None, [
            ("Write", {"file_path": "a.md", "content": "~/.config/google-chrome"}),
            ("Edit", {"file_path": "a.md", "old_string": "~/.config/google-chrome",
                      "new_string": "~/.config/google-chrome"}),
            ("MultiEdit", {"file_path": "a.md",
                           "edits": [{"old_string": "~/.config/chromium",
                                      "new_string": "~/.config/chromium"}]}),
            ("NotebookEdit", {"notebook_path": "n.ipynb", "new_source": "~/.config/chromium"}),
        ])

    # --------------------------------------------------------------- ordering

    def test_rule_order_g5_g6_then_g3_g4_g1_g2(self):
        cases = [
            # (case, expected)
            (("mcp__playwright__x", {"command": "npx playwright install"}), "G5"),
            (bash("npx playwright install ~/.config/google-chrome"), "G3"),
            (bash("agent-browser open x; npx playwright install"), "G4"),
            (bash("npx playwright install agent-browser"), "G4"),
            (bash("chromium --remote-debugging-port=1 ~/.config/chromium"), "G3"),
            (bash("npx playwright install; chromium"), "G1"),
            (bash("chromium; npx playwright install"), "G2"),
            (bash(f"echo {GH_TOKEN} > .claude/memory/a.md; npx playwright install"), "G6"),
            (bash(f"npx playwright install; echo {GH_TOKEN} > .claude/memory/a.md"), "G6"),
            (("Write", {"file_path": "~/.config/chromium/.claude/memory/a.md",
                        "content": GH_TOKEN}), "G6"),
        ]
        for case, expected in cases:
            with self.subTest(case=case):
                self.assertEqual(rule_of(case), expected)

    # ---------------------------------------------------------- shell analysis

    def test_shell_analysis_applies_to_any_tool_with_a_command(self):
        self.assert_table("G1", [
            ("Monitor", {"command": "npx playwright install"}),
            ("mcp__terminal__run", {"command": "npx playwright install"}),
        ])
        self.assert_table(None, [
            ("Monitor", {"command": "tail -f app.log"}),
        ])

    def test_home_param_controls_expansion(self):
        case = ("Read", {"file_path": "~/.config/chromium/x"})
        self.assertEqual(guard_rules.check(*case, "/home/other")[0], "G3")
        self.assertEqual(guard_rules.check(*case, "")[0], "G3")
        self.assertEqual(guard_rules.check(*case, None)[0], "G3")
        cd = bash("cd ~ && ls .config/chromium")
        self.assertEqual(guard_rules.check(*cd, "/home/other")[0], "G3")

    # -------------------------------------------------------------- interface

    def test_messages_cover_g1_to_g6_verbatim(self):
        self.assertEqual(guard_rules.MESSAGES, {
            "G1": "run it inside Docker (OrbStack); for browser work use BrowserOS Neo",
            "G2": "never launch Chrome on the host; use BrowserOS Neo",
            "G3": "Chrome profiles are off-limits",
            "G4": "agent-browser is not used here; use the BrowserOS Neo MCP tools",
            "G5": "use the BrowserOS Neo MCP tools",
            "G6": "memory is committed to the repo; never store secrets there",
        })

    def test_check_returns_rule_and_its_message(self):
        for rule, case in (("G1", bash("npx playwright install")),
                           ("G2", bash("google-chrome")),
                           ("G3", bash("ls ~/.config/chromium")),
                           ("G4", bash("agent-browser open x")),
                           ("G5", ("mcp__playwright__x", {})),
                           ("G6", ("Write", {"file_path": ".claude/memory/a.md",
                                             "content": GH_TOKEN}))):
            with self.subTest(rule=rule):
                self.assertEqual(check(case), (rule, guard_rules.MESSAGES[rule]))

    def test_self_test_sample_is_a_blocked_hook_input(self):
        sample = guard_rules.SELF_TEST_SAMPLE
        self.assertEqual(sample, {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "npx playwright install chromium"},
        })
        result = guard_rules.check(sample["tool_name"], sample["tool_input"], HOME)
        self.assertEqual(result, ("G1", guard_rules.MESSAGES["G1"]))

    def test_profile_roots_are_the_four_home_relative_paths(self):
        self.assertEqual(list(guard_rules.PROFILE_ROOTS), [
            "Library/Application Support/Google",
            "Library/Application Support/Chromium",
            ".config/google-chrome",
            ".config/chromium",
        ])

    def test_every_profile_root_is_blocked_under_home(self):
        for root in guard_rules.PROFILE_ROOTS:
            for path in (root, root + "/Default/Cookies"):
                with self.subTest(path=path):
                    self.assertEqual(
                        rule_of(("Read", {"file_path": f"{HOME}/{path}"})), "G3")
                    self.assertEqual(rule_of(bash(f"cat ~/{path.replace(' ', chr(92) + ' ')}")), "G3")

    # ----------------------------------------------------------------- robust

    def test_check_never_raises_on_odd_inputs(self):
        huge = "a" * 300_000
        odd_commands = [
            "echo 'unbalanced", 'echo "unbalanced', "echo `unbalanced", "echo $(unbalanced",
            "echo ${unbalanced", "echo $'unbalanced", "((((((", "))))))", "$(", "`", "'", '"',
            "\\", "<<", "<<EOF", "<<EOF\nbody", "cat <<EOF\nit's\nnever closed", "<<<", ">", "&>",
            "| | |", "; ; ;", "&& ||", "\x00", "\n\n\n", "$(" * 50 + ")" * 50, "`" * 99,
            "(" * 500 + "npx playwright install" + ")" * 500,
            "$(" * 200 + "npx playwright install" + ")" * 200,
            "bash -c '" * 30 + "ls" + "'" * 30,
            "cat <<EOF\nit's a heredoc with apostrophes\nEOF\necho don't",
            "echo don't; npx playwright install", "git commit -m \"it's\" && echo it's",
            "ls ~/\udcff", "ls \u202e", "sudo", "env -S", "npx", "npx -p", "python -m",
            "timeout", "xargs -I", "rtk", "rtk bypass", "docker", "docker run -c", "find -exec",
            "open -a", "open -b", "cd", "pushd", "tee", "dd of=", "sed -i", "FOO=",
            huge, huge + "\nnpx playwright install", "x " * 100_000, "\n" * 100_000,
            "echo " + "'" * 100_001,
            " && ".join(f"cd d{i}" for i in range(3000)),
            "cat <<EOF x\n" * 2000,
            "$(cat <<EOF\n" * 300,
            "echo $(" * 40 + "ls" + ")" * 40,
        ]
        for command in odd_commands:
            with self.subTest(command=command[:60]):
                guard_rules.check("Bash", {"command": command}, HOME)
        odd_inputs = [
            ("Bash", None), ("Bash", []), ("Bash", "ls"), ("Bash", 5), ("Bash", {}),
            ("Bash", {"command": None}), ("Bash", {"command": 5}), ("Bash", {"command": ["ls"]}),
            ("Bash", {"command": {"a": "b"}}), ("Bash", {"command": b"ls"}),
            (None, {"command": "ls"}), (5, {}), (["x"], {}), ({}, {}), (b"x", {}),
            ("Read", {"file_path": None}), ("Read", {"file_path": 5}),
            ("Read", {"file_path": ["~/.config/chromium"]}), ("Read", {None: "x"}),
            ("Read", {5: "x"}), ("Write", {"file_path": None, "content": None}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": None}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": 5}),
            ("Write", {"file_path": ".claude/memory/a.md", "content": [GH_TOKEN]}),
            ("Write", {"file_path": 5, "content": GH_TOKEN}),
            ("MultiEdit", {"file_path": ".claude/memory/a.md", "edits": None}),
            ("MultiEdit", {"file_path": ".claude/memory/a.md", "edits": [None, 5, "x", {}]}),
            ("MultiEdit", {"file_path": ".claude/memory/a.md", "edits": [{"new_string": None}]}),
            ("Glob", {"pattern": None, "path": 5}), ("Grep", {"pattern": ["a"], "path": {"b": 1}}),
            ("Glob", {"pattern": "*", "path": None}),
            ("Tool", {"a": {"b": {"c": {"d": [1, 2, [3, [4, {"e": "~/.config/chromium"}]]]}}}}),
            ("Tool", {"a": [[[[[[[[[[[[[[[[[[[[[["~/x"]]]]]]]]]]]]]]]]]]]]]]}),
            ("Tool", {"k" * 1000: "v" * 100_000}),
            ("Tool", {"command": "ls", "x": float("nan"), "y": True, "z": 3.5}),
        ]
        for tool, tool_input in odd_inputs:
            with self.subTest(tool=repr(tool)[:30], tool_input=repr(tool_input)[:60]):
                guard_rules.check(tool, tool_input, HOME)
        for home in (None, 5, [], {}, "", "relative", "/", "//", HOME + "/", "~", "$HOME", b"/x"):
            with self.subTest(home=repr(home)):
                guard_rules.check("Bash", {"command": "cd ~ && ls .config"}, home)
                guard_rules.check("Read", {"file_path": "~/.config/chromium"}, home)

    def test_array_comments_do_not_hide_later_commands(self):
        """A quote or `(` inside an array comment does not swallow the lines after `)` (round 4)."""
        self.assert_table("G1", [
            bash("deps=(\n  playwright  # it's the e2e runner\n)\nnpx playwright install"),
            bash("arr=( # it's\n  a\n)\nnpx playwright install"),
            bash("arr=( # \"\n  a\n)\nnpx playwright install"),
            bash("arr=( # (\n  a\n)\nnpx playwright install"),
            bash("deps=(\n x # it's\n)\nsudo npx playwright install && echo ok"),
            bash("deps=(\n x # it's\n)\n\nnpx playwright install"),
            bash("f() {\n local a=( # it's\n x\n )\n npx playwright install\n}\nf"),
            bash("arr=( # it's\n  $(echo hi # it's (\n  )\n)\nnpx playwright install"),
            bash("arr=(\n  x # c \\\n  $(npx playwright install)\n)"),
            bash("arr=(\n  x # it's \\\n)\nnpx playwright install"),
            bash("arr=(\"a\\\"b # it's\")\nnpx playwright install"),
            bash("arr=(\"${x#\\\"}\")\nnpx playwright install"),
            # `#` inside quotes, in `${x#y}` / `${x##*/}` / `${#a[@]}` or mid-word is not a comment
            bash("arr=(${x#y} $(npx playwright install))"),
            bash("arr=(${x##*/} $(npx playwright install))"),
            bash("arr=(${#arr[@]} $(npx playwright install))"),
            bash("arr=(a#b $(npx playwright install))"),
            bash("arr=(\"a # it's\" $(npx playwright install))"),
            bash("arr=('a # (' $(npx playwright install))"),
            bash("arr=(a#b)\nnpx playwright install"),
            bash("arr=(${x#y})\nnpx playwright install"),
            bash("arr=(\"a # it's\")\nnpx playwright install"),
            # round 5: array elements go through the main lexer (backticks, `${...}`, `$'...'`)
            bash("arr=(`echo a #`)\nnpx playwright install"),
            bash("arr=(`echo a # it's`)\nnpx playwright install"),
            bash("arr=(${x:-a #b})\nnpx playwright install"),
            bash("arr=(${x:-a #b} c)\nnpx playwright install"),
            bash("arr=($'it\\'s')\n# it's\nnpx playwright install"),
            bash("arr=(<(npx playwright install))"),
            bash("arr=(a) npx playwright install"),
            bash("declare -a arr=(a # it's\n b)\nnpx playwright install"),
        ])
        self.assert_table("G4", [
            bash("deps=(\n x # it's\n)\nagent-browser open https://x"),
        ])
        self.assert_table("G2", [
            bash("deps=(\n x # it's\n)\nopen -a \"Google Chrome\""),
        ])
        self.assert_table("G3", [
            bash("files=( # it's\n  a\n); cat ~/.config/chromium/Default/Cookies"),
        ])
        self.assert_table(None, [
            bash("arr=( # see (docs) and it's ok\n  a\n  b # (x\n)"),
            bash("arr=(\"a)b\" 'c)d')"),
            bash("arr=( # it's\n  a\n)\nfor x in \"${arr[@]}\"; do echo \"$x\"; done"),
            bash("arr=(\"a # it's\" b#c 'd # (')\necho \"${arr[@]}\""),
            bash("arr=(${x#y} ${x##*/} a#b)\necho ok"),
            bash("arr=( ${#arr[@]} $# )\necho ok"),
        ])

    def test_commented_array_stays_cheap(self):
        import time
        command = "arr=(\n" + "  x # it's (\n" * 10000 + ")\nnpx playwright install"
        start = time.perf_counter()
        self.assertEqual(rule_of(bash(command)), "G1")
        self.assertLess(time.perf_counter() - start, 0.5)

    def test_unterminated_heredocs_stay_cheap(self):
        """Delimiters whose terminator never appears are searched for once, not per line (N3)."""
        import time
        for name, command in (
                ("nested substitution", '"$(' * 12 + "cat <<X\n" * 2000),
                ("lexer", "cat <<X\n" * 2000),
                ("nested array", "a=(" * 12 + "cat <<X\n" * 2000)):
            with self.subTest(case=name):
                start = time.perf_counter()
                guard_rules.check("Bash", {"command": command}, HOME)
                self.assertLess(time.perf_counter() - start, 0.2)

    def test_check_survives_deep_nesting(self):
        nested = {}
        for _ in range(5000):
            nested = {"a": [nested]}
        self.assertIsNone(guard_rules.check("Tool", nested, HOME))

    def test_unbalanced_quotes_still_apply_the_rules(self):
        self.assert_table("G1", [
            bash("echo don't; npx playwright install"),
            bash("npx playwright install  # it's cheap\nls"),
        ])

    # ------------------------------------------- fix lane F2 (final review I1-I3)

    def test_rtk_command_runners_are_unwrapped(self):
        """`rtk run|err|test|summary` run a command like `rtk bypass` does (I1, F2-R1)."""
        self.assert_table("G1", [
            bash("rtk run npx playwright install"),
            bash("rtk run -- npx playwright install"),
            bash("rtk run -c 'npx playwright install'"),
            bash("rtk run --command 'npx playwright install'"),
            bash("rtk run --command='npx playwright install'"),
            bash("rtk run --shell bash -c 'npx playwright install'"),
            bash("rtk run -c 'npx playwright install' --shell zsh"),
            bash("rtk run --shell=bash --command 'cd e2e && npx playwright install'"),
            bash("rtk run -vc 'npx playwright install'"),
            bash("rtk err npx playwright install"),
            bash("rtk err --shell bash 'npx playwright install'"),
            bash("rtk err --shell=sh 'npx playwright install'"),
            bash("rtk test npx playwright test"),
            bash("rtk test ! npx playwright install"),
            bash("rtk test --shell sh 'npx playwright test'"),
            bash("rtk summary npx playwright install"),
            bash("rtk summary --shell bash 'npm ci && npx playwright install'"),
            bash("rtk -v run npx playwright install"),
            bash("rtk -u summary npx playwright install"),
            bash("rtk run --skip-env npx playwright install"),
            bash("rtk err \"npx playwright install\""),
            bash("sudo rtk run npx playwright install"),
            bash("cd e2e && rtk run -c 'npx playwright install'"),
            bash("rtk run bash -euo pipefail -c 'npx playwright install'"),
        ])
        self.assert_table("G2", [
            bash('rtk run open -a "Google Chrome"'),
            bash("rtk run -c 'open -a \"Google Chrome\"'"),
            bash("rtk err chromium --headless"),
        ])
        self.assert_table("G4", [
            bash("rtk err agent-browser open x"),
            bash("rtk run -c 'agent-browser open x'"),
            bash("rtk test --shell bash 'agent-browser open x'"),
        ])
        self.assert_table("G3", [
            bash("rtk run -c 'cat ~/.config/chromium/Default/Cookies'"),
            bash("rtk run --shell python3 -c \"open('" + HOME
                 + "/.config/chromium/Default/Cookies')\""),
            bash("rtk err 'cd ~/.config && cat chromium/Default/Cookies'"),
        ])
        self.assert_table(None, [
            bash("rtk run ls"),
            bash("rtk run -c 'git status'"),
            bash("rtk err cargo build"),
            bash("rtk test cargo test"),
            bash("rtk test -f Cargo.toml"),
            bash("rtk summary npm run build"),
            bash("rtk run --shell bash -c 'echo playwright'"),
            bash("rtk run -- git log --oneline"),
            bash("rtk err --shell bash 'cargo build 2>&1'"),
            bash("rtk test --shell sh 'pytest -q'"),
            bash("rtk run docker run img npx playwright install"),
            bash("rtk err \"cargo build --release\""),
        ])

    def test_shell_option_parser(self):
        """bash/sh/zsh/dash/ksh options in every form before `-c`, stdin or a file (I2, F2-R2)."""
        install = "'npx playwright install'"
        self.assert_table("G1", [
            bash("bash -eo pipefail -c " + install),
            bash("bash -euo pipefail -c " + install),
            bash("bash -o pipefail -eu -c " + install),
            bash("bash -eoo pipefail nounset -c " + install),
            bash("bash -ce " + install),
            bash("bash -ec " + install),
            bash("bash -c -e " + install),
            bash("bash -c -- " + install),
            bash("bash -c -euo pipefail " + install),
            bash("bash --rcfile x -c " + install),
            bash("bash --init-file x -c " + install),
            bash("bash --rcfile=x -c " + install),
            bash("bash --norc --noprofile -c " + install),
            bash("bash --login -euo pipefail -c " + install),
            bash("bash +O extglob -c " + install),
            bash("bash -O extglob -c " + install),
            bash("bash +euo pipefail -c " + install),
            bash("bash -xeuo pipefail -O extglob -c " + install),
            bash("zsh -fc " + install),
            bash("zsh --emulate sh -c " + install),
            bash("zsh -o pipefail -c " + install),
            bash("dash -ec " + install),
            bash("sh -euc " + install),
            bash("ksh -eo pipefail -c " + install),
            bash("/bin/bash -euo pipefail -c " + install),
            bash("sudo bash -euo pipefail -c " + install),
            bash("env FOO=1 bash -eu -c " + install),
            # stdin: heredocs, here-strings and pipes into a shell with options or `-s`
            bash("bash -euo pipefail <<'EOF'\nnpx playwright install\nEOF"),
            bash("bash -eo pipefail <<EOF\nnpx playwright install\nEOF"),
            bash("bash -s -- x <<EOF\nnpx playwright install\nEOF"),
            bash("bash -s x y <<'EOF'\nnpx playwright install\nEOF"),
            bash("bash -xs -- a b <<'EOF'\nls\nnpx playwright install\nEOF"),
            bash("echo 'npx playwright install' | bash -s -- x"),
            bash("bash -s <<<'npx playwright install'"),
            bash("bash -euo pipefail <<< 'npx playwright install'"),
            bash("echo 'npx playwright install' | bash -eo pipefail"),
            bash("printf 'npx playwright install\\n' | sh -eu"),
            bash("cat <<EOF | bash -euo pipefail -s -- arg\nnpx playwright install\nEOF"),
            bash("echo 'npx playwright install' | bash -euo pipefail -"),
            # the lenient (unbalanced quote) path uses the same parser
            bash("echo it's; bash -c -e 'npx playwright install'"),
            bash("echo it's; bash -c -- 'npx playwright install'"),
            bash("echo it's; bash -euo pipefail -c 'npx playwright install'"),
            bash("echo it's; bash -oc pipefail 'npx playwright install'"),
        ])
        self.assert_table("G3", [
            bash("bash -euo pipefail -c 'cat ~/.config/chromium/Default/Cookies'"),
            bash("bash -s -- x <<'EOF'\ncat ~/.config/google-chrome/Default/Cookies\nEOF"),
        ])
        self.assert_table("G4", [
            bash("bash -euo pipefail -c 'agent-browser open x'"),
            bash('docker run img bash -euo pipefail -c "agent-browser open x"'),
            bash("docker exec c bash -c -e 'agent-browser open x'"),
            bash("docker run --rm img zsh --emulate sh -c 'agent-browser open x'"),
            bash("echo 'agent-browser open x' | docker run -i img bash -eo pipefail"),
            bash("docker exec -i c bash -s -- x <<'EOF'\nagent-browser open x\nEOF"),
        ])
        self.assert_table(None, [
            bash("bash -euo pipefail script.sh"),
            bash("bash -e -o pipefail ./build.sh arg"),
            bash("bash --rcfile x script.sh"),
            bash("bash --rcfile=x script.sh"),
            bash("bash -O extglob ./x.sh"),
            bash("bash -c 'echo playwright'"),
            bash("bash -euo pipefail -c 'make test'"),
            bash("bash -s -- x <<EOF\necho hi\nEOF"),
            bash("sh - script.sh"),
            bash("bash -- script.sh arg"),
            bash("zsh --emulate sh script.zsh"),
            bash("echo 'echo hi' | bash -eo pipefail"),
            # a container may run what the host may not, whatever the shell options
            bash("docker run img bash -euo pipefail -c 'npx playwright install'"),
            bash("docker run -i img bash -eo pipefail <<EOF\nnpx playwright install\nEOF"),
            bash("echo 'npx playwright install' | docker run -i img sh -s -- x"),
        ])

    def test_g3_double_backslash_space_in_code_literals(self):
        """Inside a Python/JS/AppleScript string an escaped space is two backslashes and a
        space; it still names the profile (m1)."""
        bs = "\\\\"
        dbl = "~/Library/Application" + bs + " Support/Google/Chrome/Default/Bookmarks"
        self.assertEqual(dbl.count("\\"), 2)
        self.assert_table("G3", [
            bash("python3 -c 'import os; os.system(\"cat " + dbl + "\")'"),
            bash("python3 -c 'import subprocess; subprocess.run([\"cat\", \"" + dbl + "\"])'"),
            bash("osascript -e 'do shell script \"cat " + dbl + "\"'"),
            bash("node -e 'require(\"child_process\").execSync(\"cat " + dbl + "\")'"),
            bash("python3 -c 'open(\"~/Library/Application" + bs + "\\ Support/Google/Chrome\")'"),
        ])
        self.assert_table(None, [
            bash("python3 -c 'print(\"a" + bs + " b\")'"),
            bash("python3 -c 'open(\"~/Library/Application" + bs + " Support/Code/x\")'"),
        ])

    def test_g3_profile_paths_in_inline_code(self):
        """Interpreter code, heredocs fed to interpreters and code-valued tool keys (I3, F2-R3)."""
        mac = OTHER_HOME + "/Library/Application Support/Google/Chrome"
        lin = HOME + "/.config/google-chrome/Default/Cookies"
        self.assert_table("G3", [
            bash("python3 -c \"print(open('" + mac + "/Default/Bookmarks').read())\""),
            bash("python -c \"import sqlite3; sqlite3.connect('~/.config/chromium/Default/Cookies')\""),
            bash("python3 -Ic \"open('~/.config/chromium/Default/Cookies')\""),
            bash("python3 -c \"import os; os.path.expanduser('~/Library/Application Support/Google/Chrome')\""),
            bash("python3 -c \"from pathlib import Path; "
                 "Path.home() / 'Library/Application Support/Google/Chrome/Default/Cookies'\""),
            bash("python3 -c \"f'{home}/.config/google-chrome/Default'\""),
            bash("python3 -c 'import os; print(os.listdir(os.environ[\"HOME\"] + \"/.config/chromium\"))'"),
            bash("node -e \"require('fs').readFileSync('" + lin + "')\""),
            bash("node --eval=\"require('fs').readFileSync('" + lin + "')\""),
            bash("node -p \"require('fs').readdirSync('" + mac + "')\""),
            bash("nodejs -e \"require('fs').readFileSync('" + lin + "')\""),
            bash("ruby -e \"puts File.read('" + mac + "/Local State')\""),
            bash("perl -e 'open(F, \"" + mac + "/Default/Bookmarks\"); print <F>'"),
            bash("perl -lne 'print if /url/; BEGIN { @ARGV = (\"" + HOME
                 + "/.config/chromium/Default/Bookmarks\") }' notes.txt"),
            bash("osascript -e 'POSIX file \"" + mac + "\"'"),
            bash("deno eval \"Deno.readTextFileSync('" + lin + "')\""),
            bash("bun -e \"await Bun.file('" + lin + "').text()\""),
            bash("php -r \"echo file_get_contents('" + lin + "');\""),
            bash("uv run python -c \"open('" + lin + "')\""),
            bash("sudo python3 -c \"open('" + lin + "')\""),
            bash("cd ~/.config && python3 -c \"open('chromium/Default/Cookies')\""),
            # heredoc / here-string / echo text fed to an interpreter
            bash("python3 - <<'EOF'\nimport sqlite3\ndb = sqlite3.connect('" + mac
                 + "/Default/Cookies')\nEOF"),
            bash("python3 - <<EOF\nprint(open(\"$HOME/.config/chromium/Default/Bookmarks\").read())\nEOF"),
            bash("python3 <<'PY'\nimport os\np = os.path.expanduser(\"~/.config/chromium\")\nPY"),
            bash("cat <<EOF | python3\nopen('~/.config/chromium/Default/Cookies')\nEOF"),
            bash("python3 <<< \"open('" + lin + "')\""),
            bash("echo \"require('fs').readFileSync('" + lin + "')\" | node"),
            bash("printf '%s' \"open('" + lin + "')\" | python3"),
            bash("node <<'EOF'\nconst p = `${process.env.HOME}/.config/google-chrome/Default`;\nEOF"),
            bash("python3 - <<'EOF'\nsrc = '''\n" + mac + "/Default/Cookies\n'''\nEOF"),
            # escaped space and globs inside code
            bash("python3 -c \"import os; "
                 "os.listdir(os.path.expanduser('~/Library/Application\\ Support/Google'))\""),
            bash("python3 -c \"import glob, os; "
                 "glob.glob(os.path.expanduser('~/.config/chrom*/Default/Cookies'))\""),
            bash("python3 -c \"import glob; glob.glob('" + OTHER_HOME
                 + "/Library/Application Support/G*/Chrome/*/Cookies')\""),
            # rtk / prefixes / interpreters behind `--shell`
            bash("rtk run --shell python3 -c \"open('" + lin + "')\""),
        ])
        self.assert_table(None, [
            bash("python3 -c 'print(1)'"),
            bash("node -e \"console.log(path)\""),
            bash("python3 -c \"print('chromium')\""),
            bash("python3 -c \"import os; print(os.environ['HOME'])\""),
            bash("node -e \"console.log('Google Chrome is not used here')\""),
            bash("ruby -e 'puts \"Library\"'"),
            bash("perl -e 'print \"Application Support\\n\"'"),
            bash("python3 -c \"open('Library/notes.txt')\""),
            bash("python3 -c \"print('~/Library/Application Support/Code')\""),
            bash("python3 -c \"print('.config/git')\""),
            bash("python3 -c \"import json; print(json.load(open('package.json'))['name'])\""),
            bash("node -e \"require('fs').readFileSync('docs/google-chrome-notes.md')\""),
            bash("python3 -c \"print('Google', 'Chrome', 'Library', 'Application Support')\""),
            bash("python3 - <<'EOF'\n# chromium runs only via playwright in Docker; it's documented.\n"
                 "print('playwright and chromium, Google Chrome')\nEOF"),
            bash("cat <<'EOF' | python3\nprint(\"Library/Application Support is a folder\")\nEOF"),
            bash("python3 script.py --profile Default"),
            bash("perl -pi -e 's/chromium/chrome/' docs/notes.md"),
            # shell words that are not interpreter code stay as today
            bash("git commit -m 'fix Library/Application Support/Google handling'"),
            bash("cat <<EOF > notes.md\nprofiles live in ~/.config/chromium/Default\nEOF"),
            bash("echo 'open(\"~/.config/chromium\")' > notes.md"),
            # tool keys that are not code
            ("Agent", {"prompt": "python3 -c \"open('~/.config/chromium/Default/Cookies')\" is blocked"}),
            ("mcp__browseros-neo__run", {"script": "await browser.navigate('https://example.com')"}),
            ("mcp__browseros-neo__evaluate_in_page", {"expression": "document.title"}),
            ("mcp__browseros-neo__run", {"script": "return 'chromium Google Chrome Library'"}),
        ])

    def test_g3_code_keys_in_tools(self):
        """`script` / `code` / `expression` values are code: BrowserOS Neo `run` and the like."""
        url = "file://" + OTHER_HOME + "/Library/Application%20Support/Google/Chrome/Default/Bookmarks"
        self.assert_table("G3", [
            ("mcp__browseros-neo__run", {"script": "await browser.navigate('" + url + "')"}),
            ("mcp__browseros-neo__evaluate_in_page",
             {"expression": "fetch('" + url + "').then(r => r.text())"}),
            ("mcp__browseros-neo__run",
             {"script": "const p = '~/.config/chromium/Default/Cookies';\nreturn p"}),
            ("SomeTool", {"code": "open('" + HOME + "/.config/google-chrome/Default/Cookies')"}),
            ("SomeTool", {"params": {"script": "x = `" + HOME + "/.config/chromium`"}}),
        ])

    def test_g3_percent_encoded_file_urls(self):
        """`file:` URLs are percent-decoded before matching; other URLs are not (I3, F2-R4)."""
        mac = OTHER_HOME + "/Library/Application%20Support/Google/Chrome/Default/Bookmarks"
        nav = "mcp__browseros-neo__navigate"
        self.assert_table("G3", [
            (nav, {"url": "file://" + mac}),
            (nav, {"url": "file://localhost" + mac}),
            (nav, {"url": "FILE://" + mac}),
            (nav, {"url": "File://" + mac}),
            (nav, {"url": "file://" + mac.replace("/", "%2F")}),
            (nav, {"url": "file://" + OTHER_HOME + "/Library/Application%20Support/Google"}),
            (nav, {"url": "file:///home/someone/.config/%63hromium/Default"}),
            ("WebFetch", {"url": "file://" + mac, "prompt": "x"}),
            bash("open 'file://" + mac + "'"),
            bash("curl file://" + mac),
            bash("curl --url=file://" + mac),
            bash("url=file://" + mac),
            bash("url=file://" + mac + " && curl \"$url\""),
            bash("python3 -c \"import urllib.request as u; u.urlopen('file://" + mac + "')\""),
        ])
        self.assert_table(None, [
            ("WebFetch", {"url": "https://www.google.com/search?q=Library%2FApplication%20Support%2FGoogle",
                          "prompt": "x"}),
            (nav, {"url": "https://www.google.com/search?q=Library%2FApplication%20Support%2FGoogle"}),
            (nav, {"url": "https://example.com/Library/Application%20Support/Google"}),
            (nav, {"url": "file:///tmp/a%20b.html"}),
            (nav, {"url": "file://" + HOME + "/proj/Application%20Support/index.html"}),
            (nav, {"url": "file://" + OTHER_HOME + "/Library/Application%20Support/Code/x.html"}),
            bash("curl 'https://example.com/?q=.config%2Fchromium'"),
            bash("open file:///tmp/a%20b.html"),
        ])

    def test_inline_code_scan_stays_cheap(self):
        """A 200 KB `-c` program and a 10,000-line heredoc into python3 scan well under 1 s."""
        import time
        line = "x = os.path.join('src', 'pkg/mod_%d.py'); print('done', x)\n"
        program = "".join(line % k for k in range(4000))
        self.assertGreater(len(program), 200_000)
        heredoc = "python3 - <<'EOF'\n" + "".join(line % k for k in range(10_000)) + "EOF"
        for name, command, expected in (
                ("-c program", "python3 -c \"" + program + "\"", None),
                ("-c program + path", "python3 -c \"" + program
                 + "open('~/.config/chromium/Default/Cookies')\"", "G3"),
                ("heredoc", heredoc, None),
                ("heredoc + path", heredoc[:-3] + "open('~/.config/chromium/x')\nEOF", "G3")):
            with self.subTest(case=name):
                start = time.perf_counter()
                self.assertEqual(rule_of(bash(command)), expected)
                self.assertLess(time.perf_counter() - start, 1.0)

    def test_g3_quote_in_a_comment_does_not_hide_a_code_literal(self):
        """A stray apostrophe (`# Chrome's ...`) must not pair with a path literal's quote (C1)."""
        mac = "Library/Application Support/Google/Chrome/Default"
        sqlite = ("import sqlite3\n# it's the cookie db\nc = sqlite3.connect('~/" + mac
                  + "/Cookies')  # don't commit\n")
        self.assert_table("G3", [
            bash("python3 - <<'EOF'\nimport json, os\n# Chrome's bookmarks live here\n"
                 "path = os.path.expanduser('~/" + mac + "/Bookmarks')\n"
                 "print(json.load(open(path))['roots'].keys())\nEOF"),
            bash("python3 - <<'EOF'\n# don't modify, read only\nimport shutil\n"
                 "shutil.copy('" + OTHER_HOME + "/" + mac + "/Cookies', '/tmp/c')\nEOF"),
            bash("node <<'EOF'\n// don't write, it's read only\n"
                 "const p = require('os').homedir() + '/" + mac + "/Bookmarks';\nEOF"),
            bash("python3 -c \"# it's here\nopen('" + OTHER_HOME + "/" + mac + "/Cookies')\""),
            ("mcp__ide__executeCode", {"code": sqlite}),
            ("mcp__browseros-neo__run", {"code": sqlite}),
        ])
        self.assert_table(None, [
            bash("python3 - <<'EOF'\n# Chrome's profile is never read here; it's off-limits\n"
                 "print('don\\'t', \"it's\")\nEOF"),
            ("mcp__browseros-neo__run", {"code": "// it's fine\nawait browser.navigate('https://x.dev')"}),
        ])

    def test_g3_save_skill_fn_is_code(self):
        """Neo `save_skill` runs `flow[].fn` / `patch[].fn` to verify a skill, so they are code (m2)."""
        url = "file://" + OTHER_HOME + "/Library/Application%20Support/Google/Chrome/Default/Bookmarks"
        tool = "mcp__browseros-neo__save_skill"
        self.assert_table("G3", [
            (tool, {"name": "x", "flow": [{"fn": "async () => goto('" + url + "')"}]}),
            (tool, {"name": "x", "patch": [{"fn": "() => fetch('" + url + "')"}]}),
            (tool, {"name": "x", "flow": [{"step": 1, "fn": "() => fetch('file://" + HOME
                                           + "/.config/chromium/Default/Cookies')"}]}),
        ])
        self.assert_table(None, [
            (tool, {"name": "x", "flow": [{"fn": "async () => document.title"}]}),
            (tool, {"name": "x", "patch": [{"fn": "() => 1", "note": url}]}),
        ])

    def test_g3_browseros_neo_real_code_keys(self):
        """BrowserOS Neo's real shapes: `run {code}`, `evaluate_in_page {page, code|func}` (M1)."""
        url = "file://" + OTHER_HOME + "/Library/Application%20Support/Google/Chrome/Local%20State"
        self.assert_table("G3", [
            ("mcp__browseros-neo__run", {"code": "await browser.navigate('" + url + "')"}),
            ("mcp__browseros-neo__evaluate_in_page", {"page": 1, "code": "fetch('" + url + "')"}),
            ("mcp__browseros-neo__evaluate_in_page", {"page": 1, "func": "() => fetch('" + url + "')"}),
            ("mcp__browseros-neo__evaluate_in_page",
             {"page": 1, "func": "() => fetch('file://" + HOME + "/.config/chromium/Default/Bookmarks')"}),
        ])
        self.assert_table(None, [
            ("mcp__browseros-neo__evaluate_in_page", {"page": 1, "func": "() => document.title"}),
            ("mcp__browseros-neo__evaluate_in_page", {"page": 1, "code": "location.href"}),
        ])

    def test_bad_glob_ranges_do_not_fail_open(self):
        """A reversed glob range (`[z-a]`) is no match, not an exception that allows all (M3)."""
        self.assert_table("G1", [
            bash("ls '[z-a]/b/c' && npx playwright install"),
            bash("python3 -c \"x = '[z-a]/b/c'\" && npx playwright install"),
            bash("cat ~/.config/[z-a]*/x; npx playwright install"),
        ])
        self.assert_table(None, [
            bash("ls '[z-a]/b/c'"),
            ("Glob", {"pattern": "[z-a]/**", "path": "src"}),
        ])

    def test_more_interpreters_and_fish(self):
        """`pypy*`, `tsx`, `ts-node` run inline code; `fish -c` is a shell (M6)."""
        lin = HOME + "/.config/google-chrome/Default/Cookies"
        self.assert_table("G3", [
            bash("pypy3 -c \"open('" + lin + "')\""),
            bash("pypy -c \"open('" + lin + "')\""),
            bash("tsx -e \"require('fs').readFileSync('" + lin + "')\""),
            bash("npx tsx -e \"require('fs').readFileSync('" + lin + "')\""),
            bash("ts-node -e \"require('fs').readFileSync('" + lin + "')\""),
            bash("echo \"require('fs').readFileSync('" + lin + "')\" | ts-node"),
            bash("fish -c 'cat ~/.config/chromium/Default/Cookies'"),
        ])
        self.assert_table("G1", [
            bash("fish -c 'npx playwright install'"),
            bash("fish --no-config -c 'npx playwright install'"),
            bash("rtk run --shell fish -c 'npx playwright install'"),
            bash("echo 'npx playwright install' | fish"),
        ])
        self.assert_table(None, [
            bash("pypy3 -c 'print(1)'"),
            bash("tsx src/index.ts"),
            bash("ts-node -e \"console.log('chromium')\""),
            bash("fish -c 'echo playwright'"),
            bash("fish script.fish"),
        ])

    def test_fish_is_parsed_with_fish_options(self):
        """fish takes `--command=X`, `-C X` / `--init-command X` (also run) and attached values;
        a bash parse misses them (m3)."""
        for script, rule in (("npx playwright install", "G1"),
                             ("cat ~/.config/chromium/Default/Cookies", "G3")):
            q = "'" + script + "'"
            self.assert_table(rule, [
                bash("fish --command=" + q),
                bash("fish --command " + q),
                bash("fish --comm=" + q),
                bash("fish -C " + q + " -c exit"),
                bash("fish --init-command " + q + " -c exit"),
                bash("fish --init-command=" + q + " -c exit"),
                bash("fish --ini=" + q),
                bash("fish -C" + q),
                bash("fish -c" + q),
                bash("fish -ic " + q),
                bash("fish -Pc " + q),
                bash("fish -d all -c " + q),
                bash("fish --no-config --command=" + q),
                bash("sudo fish --command=" + q),
                bash("rtk run --shell fish -c " + q),
                bash("fish -C " + q + " <<< x"),
            ])
        self.assert_table("G1", [
            bash("echo 'npx playwright install' | fish -C 'echo hi'"),
        ])
        self.assert_table("G3", [
            bash("docker run img fish --command='cat ~/.config/chromium/Default/Cookies'"),
            bash("docker run img fish -C 'cat ~/.config/chromium/Default/Cookies' -c exit"),
            bash("echo 'cat ~/.config/chromium/Default/Cookies' | docker run -i img fish"),
        ])
        self.assert_table(None, [
            bash("fish --command='echo playwright'"),
            bash("fish -C 'echo hi' -c exit"),
            bash("fish -c 'echo hi' 'npx playwright install'"),      # extra words are $argv
            bash("fish --no-config script.fish"),
            bash("fish --version"),
            bash("docker run img fish --command='echo hi'"),
        ])

    def test_fish_fails_closed_past_unknown_options(self):
        """A fish value option the parser does not know (`--debug-stack-frames N`) must not hide
        a later `-c`: every word is also scanned for a script form (fix round 1, I1)."""
        for script, rule in (("npx playwright install", "G1"),
                             ("cat ~/.config/chromium/Default/Cookies", "G3")):
            q = "'" + script + "'"
            self.assert_table(rule, [
                bash("fish --debug-stack-frames 3 -c " + q),
                bash("fish --debug-stack 3 -c " + q),
                bash("fish --debug-stack-frames=3 -c " + q),
                bash("fish --debug-stack-frames 3 --command=" + q),
                bash("fish --debug-stack-frames 3 -C " + q),
                bash("fish --login -c " + q),
                bash("fish --private -c " + q),
                bash("fish --no-config -c " + q),
                bash("fish --print-rusage-self -c " + q),
                bash("fish --some-future-flag value -c " + q),
                bash("fish --future-flag value --command=" + q),
                bash("fish -d -c " + q),
                bash("fish script.fish -c " + q),
                bash("fish script.fish --command=" + q),
                bash("fish -- -c " + q),
                bash("fish --c=" + q),
                bash("fish --co " + q),
                bash("fish --private --command " + q),
                bash("fish -nc " + q),
                bash("fish -N -c " + q),
                bash("fish -C " + q + " script.fish"),
                bash("fish --init-command=" + q + " script.fish"),
                bash("fish -c 'echo a' -c " + q),
                bash("fish -o f -c " + q),
                bash("fish --debug-output f -c " + q),
                bash("fish -D 3 -c " + q),
                bash("fish --profile-startup p -c " + q),
                bash("fish --features x -c " + q),
                bash("exec fish -c " + q),
                bash("env fish --command=" + q),
                bash("xargs fish -c " + q),
                # unbalanced quote: the lenient lexer builds the script from the rest of the line
                bash("fish --command=" + q + "; echo \""),
                bash("fish -c" + q + "; echo \""),
                bash("fish -C " + q + " -c exit; echo \""),
            ])
        # fish reading its script from stdin: a value option (known or not) must not turn the
        # piped text into "a script file's argument" (fix FW-Q).
        piped = "echo 'npx playwright install' | "
        self.assert_table("G1", [
            bash(piped + "fish --debug-stack-frames 3"),
            bash(piped + "fish --debug-stack 3"),
            bash(piped + "fish --debug-stack-frames=3"),
            bash(piped + "fish --debug-output f"),
            bash(piped + "fish --some-future-flag value"),
            bash(piped + "fish --private --another-future-flag value"),
            bash(piped + "fish --login"),
        ])
        # the table of value options is not what keeps this closed: without it the form still holds
        with mock.patch.object(guard_rules, "_FISH_LONG_VALUE", ("--command", "--init-command")):
            self.assert_table("G1", [
                bash(piped + "fish --debug-stack-frames 3"),
                bash(piped + "fish --debug-stack-frames=3"),
                bash(piped + "fish --debug-output f"),
            ])
        self.assert_table(None, [
            bash(piped + "fish script.fish"),
            bash(piped + "fish --login script.fish"),
            bash(piped + "fish --no-config script.fish extra"),
        ])
        self.assert_table("G3", [       # docker exempts G1 and G2, not G3
            bash("docker run img fish --debug-stack-frames 3 -c 'cat ~/.config/chromium/Default/Cookies'"),
        ])
        self.assert_table(None, [
            bash("fish --login -c 'echo hi'"),
            bash("fish --debug-stack-frames 3 -c 'echo hi'"),
            bash("fish script.fish --arg"),
            bash("fish --no-config script.fish extra"),
            bash("fish -c 'echo hi' --private"),
        ])

    def test_code_candidate_prefilter_stays_cheap(self):
        """Many unique literals / path-like words in code scan well under 1 s (M4)."""
        import time
        literals = "".join("v%d = 'lit%d/a/b/c'\n" % (k, k) for k in range(60_000))
        words = " ".join("w%d/x/y/z/q/r/s" % k for k in range(200_000))
        for name, command, expected in (
                ("60k unique literals", "python3 - <<'EOF'\n" + literals + "EOF", None),
                ("60k unique literals + path", "python3 - <<'EOF'\n" + literals
                 + "open('~/.config/chromium/x')\nEOF", "G3"),
                ("200k path-like words", "python3 -c '" + words + "'", None),
                ("200k path-like words + url", "python3 -c '" + words + " file:%2Fhome%2Ftester"
                 "%2F.config%2Fchromium'", "G3")):
            with self.subTest(case=name):
                start = time.perf_counter()
                self.assertEqual(rule_of(bash(command)), expected)
                self.assertLess(time.perf_counter() - start, 1.0)

    def test_escaped_space_normalising_scales_linearly(self):
        """A long backslash run with no space after it costs time proportional to its length: 4x
        the input costs under 6x (fix round 1, I2). Best of three, so a busy host does not trip it."""
        import time

        def cost(count):
            command = "python3 -c '" + "\\" * count + "'"
            best = None
            for _ in range(3):
                start = time.perf_counter()
                self.assertIsNone(rule_of(bash(command)))
                elapsed = time.perf_counter() - start
                best = elapsed if best is None else min(best, elapsed)
            return best

        small, large = cost(50_000), cost(200_000)
        self.assertLess(large, 6 * max(small, 0.005))
        # the run still normalises when a space ends it
        bs = "\\" * 50_000
        self.assertEqual(rule_of(bash(
            "python3 -c 'open(\"~/Library/Application" + bs + " Support/Google/Chrome\")'")), "G3")

    def test_check_is_deterministic_and_pure(self):
        tool_input = {"command": "npx playwright install"}
        first = guard_rules.check("Bash", tool_input, HOME)
        second = guard_rules.check("Bash", tool_input, HOME)
        self.assertEqual(first, second)
        self.assertEqual(tool_input, {"command": "npx playwright install"})


if __name__ == "__main__":
    unittest.main()
