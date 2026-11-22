//! P3: `classify_readonly` proves a Bash command is read-only, and anything it does not prove is not.
//!
//! The adversarial table is the point of this file: every shape that writes, runs another program, expands at
//! run time, leaves the directory the path check will look at, or that the classifier merely does not know is
//! `NotProven`. The pass table is deliberately plain. A classifier bug is a silent allow, so a command that
//! is only probably read-only belongs in the first table.

use anthology_shell::{
    READONLY_PROGRAMS, ReadOnly, SECRET_PATTERNS, analyze, classify_readonly, head_tail_count,
    is_secret_path, literal_cd_target,
};
use proptest::prelude::*;

const HOME: &str = "/home/tester";

fn classify(command: &str) -> ReadOnly {
    classify_readonly(command, HOME)
}

/// The paths (as written) and the `cd` targets of a proven command.
#[track_caller]
fn proven(command: &str) -> (Vec<String>, Vec<String>) {
    let (paths, cd, _) = proven_full(command);
    (paths.into_iter().map(|p| p.0).collect(), cd)
}

/// Every field of a proven command: each path with its `recursive` flag, the `cd` targets, `runs_git_config`.
#[track_caller]
fn proven_full(command: &str) -> (Vec<(String, bool)>, Vec<String>, bool) {
    match classify(command) {
        ReadOnly::Proven {
            paths,
            cd,
            runs_git_config,
        } => (
            paths.into_iter().map(|p| (p.path, p.recursive)).collect(),
            cd,
            runs_git_config,
        ),
        ReadOnly::NotProven(why) => panic!("{command:?} should be proven, got NotProven({why})"),
    }
}

#[track_caller]
fn paths_of(command: &str) -> Vec<String> {
    proven(command).0
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[track_caller]
fn assert_none_proven(commands: &[&str]) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|c| match classify(c) {
            ReadOnly::Proven { paths, cd, .. } => {
                Some(format!("{c:?}: Proven {paths:?} cd {cd:?}"))
            }
            ReadOnly::NotProven(_) => None,
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "these must not be proven read-only:\n{}",
        wrong.join("\n")
    );
}

// --- what must pass ---------------------------------------------------------------------------------------

#[test]
fn plain_read_only_commands_are_proven() {
    for command in [
        "ls",
        "ls -la",
        "ls -la src tests",
        "cat README.md",
        "cat a.txt b.txt",
        "head -n 20 src/main.rs",
        "head -20 src/main.rs",
        "tail -n 5 log.txt",
        "wc -l src/main.rs",
        "grep -rn 'foo.*bar' src",
        "grep -e foo -e bar a.txt b.txt",
        "grep -c needle haystack.txt",
        "egrep -i 'a|b' a.txt",
        "fgrep -n 'a.b' a.txt",
        "rg -n TODO src tests",
        "rg --files",
        "rg -t rust -g '*.toml' pattern",
        "find . -name '*.rs' -type f",
        "find src -maxdepth 2 -name '*.toml' -newer Cargo.lock",
        "find . -type f -size +1M -print",
        "stat -c %s README.md",
        "file --mime-type logo.png",
        "du -sh .",
        "du -h -d 1 src",
        "pwd",
        "which cargo",
        "basename /a/b.txt .txt",
        "dirname src/lib.rs",
        "realpath .",
        "tree -L 2 src",
        "sort a.txt",
        "sort -u -k 2 a.txt",
        "uniq -c a.txt",
        "cut -d, -f1 a.csv",
        "diff a.txt b.txt",
        "diff -u a.txt b.txt",
        "jq '.name' package.json",
        "jq -r '.[] | .id' a.json",
        "jq '.environment' a.json",
        "jq '.[] | select(.name == \"x\") | .envelope' a.json",
        "jq '.a.b | length' a.json",
        "ls | wc -l",
        "cat a.txt | grep foo | sort | uniq -c",
        "cat a.txt | tr a-z A-Z",
        "ls -la /tmp | head -n 5",
        "ls && ls src",
        "grep foo a.txt && wc -l a.txt",
    ] {
        proven(command);
    }
}

#[test]
fn git_read_commands_are_proven() {
    for command in [
        "git status",
        "git status -sb",
        "git status --porcelain=v1 -uno",
        "git log",
        "git log --oneline -5",
        "git log --oneline --graph --decorate -n 10",
        "git log --pretty=format:%h -- src",
        "git log -p -- src/lib.rs",
        "git log --since=2.weeks --author=bob",
        "git diff",
        "git diff --stat",
        "git diff --cached --name-only",
        "git diff main src",
        "git diff -- src",
        "git show --stat HEAD",
        "git show --no-patch --format=%H HEAD",
        "git blame -L 1,10 src/lib.rs",
        "git blame -w -M src/lib.rs",
        "git ls-files",
        "git ls-files --others --exclude-standard src",
        "git rev-parse --show-toplevel",
        "git rev-parse --abbrev-ref HEAD",
        "git describe --tags",
        "git describe --tags --abbrev=0",
        "git branch",
        "git branch -a",
        "git branch -vv",
        "git branch --show-current",
        "git branch --list",
        "git branch --contains HEAD",
        "git remote",
        "git remote -v",
        "git --no-pager log -3",
        "git --no-optional-locks status",
        "git diff -- src/lib.rs | head -n 20",
    ] {
        proven(command);
    }
}

#[test]
fn the_paths_a_command_names_are_reported_as_written() {
    assert_eq!(paths_of("ls"), strs(&["."]));
    assert_eq!(paths_of("ls -la src tests"), strs(&["src", "tests"]));
    assert_eq!(
        paths_of("cat a.txt b.txt | wc -l"),
        strs(&["a.txt", "b.txt"])
    );
    assert_eq!(paths_of("wc -l src/*.rs"), strs(&["src/*.rs"]));
    // the first operand of grep and rg is the pattern, the rest are paths
    assert_eq!(paths_of("grep -rn 'a b' src"), strs(&["src"]));
    assert_eq!(paths_of("grep foo"), strs(&["."]));
    assert_eq!(paths_of("grep -e foo a b"), strs(&["a", "b"]));
    assert_eq!(paths_of("grep -f pats.txt a"), strs(&["pats.txt", "a"]));
    assert_eq!(paths_of("rg --files src"), strs(&["src"]));
    // a find start point, and the file `-newer` reads
    assert_eq!(paths_of("find . -name x"), strs(&["."]));
    assert_eq!(
        paths_of("find src -newer Cargo.lock -name x"),
        strs(&["src", "Cargo.lock"])
    );
    // a pattern, a number, a delimiter and a program name are not paths
    assert_eq!(paths_of("head -n 5 a"), strs(&["a"]));
    assert_eq!(paths_of("cut -d, -f1 a.csv"), strs(&["a.csv"]));
    assert_eq!(paths_of("jq '.a' x.json"), strs(&["x.json"]));
    assert_eq!(paths_of("tr a-z A-Z"), Vec::<String>::new());
    assert_eq!(paths_of("basename /a/b.txt"), Vec::<String>::new());
    assert_eq!(paths_of("which cargo"), Vec::<String>::new());
    assert_eq!(paths_of("pwd"), Vec::<String>::new());
    // after `--` every word is a path
    assert_eq!(paths_of("cat -- -x"), strs(&["-x"]));
    // a read redirect names a path; the allowed output redirects name none
    assert_eq!(paths_of("cat < a.txt"), strs(&["a.txt"]));
    assert_eq!(paths_of("ls 2>&1"), strs(&["."]));
    assert_eq!(paths_of("ls >/dev/null"), strs(&["."]));
    assert_eq!(paths_of("ls 2>/dev/null"), strs(&["."]));
    assert_eq!(paths_of("ls &>/dev/null"), strs(&["."]));
    assert_eq!(paths_of("ls -la 2>&1 | head -n 5"), strs(&["."]));
    // git: every operand may be a path, a word after `--` always is
    assert_eq!(paths_of("git status"), Vec::<String>::new());
    assert_eq!(
        paths_of("git log --oneline -5 main src"),
        strs(&["main", "src"])
    );
    assert_eq!(
        paths_of("git log --oneline -5 -- src a.rs"),
        strs(&["src", "a.rs"])
    );
    assert_eq!(paths_of("git diff --stat"), Vec::<String>::new());
}

#[test]
fn a_leading_cd_is_reported_and_the_rest_runs_after_it() {
    assert_eq!(
        proven("cd /work/proj && ls"),
        (strs(&["."]), strs(&["/work/proj"]))
    );
    assert_eq!(
        proven("cd ./sub && git status"),
        (Vec::<String>::new(), strs(&["./sub"]))
    );
    assert_eq!(
        proven("cd /a && cd ./b && cat k.txt | head -n 3"),
        (strs(&["k.txt"]), strs(&["/a", "./b"]))
    );
    assert_eq!(
        proven("cd /w && ls src && ls tests"),
        (strs(&["src", "tests"]), strs(&["/w"]))
    );
    // the operators need no spaces round them
    for command in ["cd /a&&ls", "cd /a&& ls", "cd /a &&ls"] {
        assert_eq!(proven(command), (strs(&["."]), strs(&["/a"])), "{command}");
    }
    // a redirect reads a path after the cd, like any other operand
    assert_eq!(
        proven("cd /w && cat < k.txt"),
        (strs(&["k.txt"]), strs(&["/w"]))
    );
}

// --- redirections -----------------------------------------------------------------------------------------

#[test]
fn a_redirect_that_can_write_a_file_is_not_proven() {
    assert_none_proven(&[
        "ls > out.txt",
        "ls >out.txt",
        "ls 1> out.txt",
        "ls 2> err.txt",
        "ls 3> out.txt",
        "ls >> out.txt",
        "ls >| out.txt",
        "ls &> out.txt",
        "ls &>> out.txt",
        "ls >&out.txt",
        "ls 2>&out.txt",
        "ls >&-",
        "ls >&3",
        "ls 2>&3",
        "ls >&",
        "ls >",
        "ls 2>",
        "ls > /dev/null.txt",
        "ls > /dev/nul",
        "ls > /dev//null",
        "ls > ./dev/null",
        "ls > /dev/null/x",
        "ls > /dev/tcp/127.0.0.1/80",
        "ls > /dev/stdout",
        "ls > /dev/fd/1",
        "ls >/dev/null >out.txt",
        "ls >out.txt >/dev/null",
        "ls >/dev/null 2>out.txt",
        "ls > *",
        "ls > /dev/nul?",
        "cat <> f",
        "cat <>f",
        "cat <&3",
        "cat <&0",
        "cat < /dev/null > f",
        "cat a.txt > b.txt",
        "cat a.txt | cat > b.txt",
        "git log > out.txt",
        "git diff >patch.diff",
        "git status 2>>err",
        "grep foo a.txt >matches",
        "tail -n 5 log >> copy",
    ]);
}

#[test]
fn an_input_redirect_other_than_a_literal_path_is_not_proven() {
    assert_none_proven(&[
        "cat <<EOF\nx\nEOF",
        "cat <<'EOF'\nx\nEOF",
        "cat <<-EOF\nx\nEOF",
        "cat <<< x",
        "cat <<< $(ls)",
        "cat < $X",
        "cat < \"$X\"",
        "cat < $(ls)",
        "cat < <(ls)",
        "cat <(ls)",
        "cat < *.txt",
        "cat < ~/x",
        "cat < ../x",
        "cat <",
        "cat < -",
        "cat <1-3>",
        "ls f<1-3>",
        "ls > > /dev/null",
        "ls < < a",
        "ls > < a",
        "cat < > /dev/null",
        "cat < a < b >c",
    ]);
}

// --- substitutions, expansions and the shapes of #133 -------------------------------------------------------

#[test]
fn a_substitution_or_expansion_is_not_proven() {
    assert_none_proven(&[
        "ls $(pwd)",
        "ls \"$(pwd)\"",
        "ls '$(pwd)' $(pwd)",
        "ls `pwd`",
        "ls \"`pwd`\"",
        "ls <(ls)",
        "ls >(cat)",
        "ls \"<(ls)\" <(ls)",
        "cat <(ls)",
        "cat >(tee x)",
        "ls $X",
        "ls ${X}",
        "ls ${X:-.}",
        "ls \"$X\"",
        "ls \"${X}\"",
        "ls $HOME",
        "ls \"$HOME/src\"",
        "ls $'a'",
        "ls $\"a\"",
        "ls $'\\x41'",
        "ls $$",
        "ls $?",
        "ls $0",
        "ls $@",
        "ls $*",
        "ls $#",
        "ls $-",
        "ls $!",
        "ls \"a$\"",
        "ls \"a`\"",
        "ls a$",
        "ls ${!x}",
        "ls $[1+1]",
        "ls $((1+1))",
        "ls \"$((1+1))\"",
        "$(echo ls)",
        "`echo ls`",
        "$(echo ls) -la",
        "`echo ls` -la",
        "\"$(echo ls)\" -la",
        "git $(echo status)",
        "git status $(echo -s)",
        "git `echo log`",
        "cat $(ls)",
        "grep $(echo x) a.txt",
        "find . -name $(echo x)",
        "find $(pwd) -name x",
        "cd $(pwd) && ls",
        "cd `pwd` && ls",
        "cd /a && ls $(pwd)",
        "ls | grep $(whoami)",
        "ls | head -n $(echo 5)",
        "ls | head -n \"$(echo 5)\"",
        "ls | head -n $N",
        "ls | head -n \"$N\"",
        "ls | head -n ${N}",
    ]);
}

/// The shapes of the DT-4 fix (#133): a program word hidden in a `$(`, or in a substitution inside `$((`.
#[test]
fn the_arithmetic_holder_shapes_are_not_proven() {
    assert_none_proven(&[
        "echo $(( $(`echo agent-browser` open x) ))",
        r#"echo $(( $($(echo open) -a "Google Chrome") ))"#,
        "echo $(( 1 + $(`echo agent-browser` open x) ))",
        "x=$(( $(`echo agent-browser` open x) ))",
        "echo $(( $(true; `echo agent-browser` open x) ))",
        "echo $(( $(( $(( $(`echo agent-browser` open x) )) )) ))",
        "echo $(( $(echo $(( $(`echo agent-browser` open x) )) ) ))",
        "echo $(( `\\`echo agent-browser\\` open x` ))",
        "echo $(( a[$(`echo agent-browser` open x)] ))",
        "echo $(( $[ $(`echo agent-browser` open x) ] ))",
        "echo $(( $(cat <(`echo agent-browser` open x)) ))",
        "echo $(((echo a) ; `echo agent-browser` open x) )",
        "echo $(((1)) ; `echo agent-browser` open x)",
        r#"echo $(((1)) ; $(echo open) -a "Google Chrome")"#,
        "x=$(((echo a); `echo agent-browser` open x) )",
        "$(((true)); echo agent-browser) open x",
        "\"$(((true)); echo agent-browser)\" open x",
        r#"$(((true)) ; echo open) -a "Google Chrome""#,
        "$(((1)) ; echo npx) playwright install",
        // the same shapes with a read-only program in front, which is what makes them dangerous here
        "ls $(( $(`echo agent-browser` open x) ))",
        "ls $(( $($(echo open) -a x) ))",
        "ls $(( 1 + $(ls) ))",
        "ls $(( `ls` ))",
        "ls $(((true)); echo agent-browser)",
        "cat $(((1)) ; `echo agent-browser` open x)",
        "git status $(((1)) ; ls)",
        "ls $((1))",
        "ls $(((1)))",
        "ls $((1)",
        "ls $(((1))",
        "ls $(( 1",
        "ls $(",
        "ls `",
        "cat <(cat <(ls))",
        "cat $(cat $(ls))",
        // the neighbours outside `$((`
        "`echo agent-browser` open x",
        r#"$(echo open) -a "Google Chrome""#,
    ]);
}

// --- every program the table does not know --------------------------------------------------------------

#[test]
fn a_program_outside_the_table_is_not_proven() {
    assert_none_proven(&[
        "",
        "   ",
        "echo hi",
        "printf x",
        "tee out.txt",
        "ls | tee out.txt",
        "ls | tee -a out.txt",
        "git log | tee out.txt",
        "sed -i s/a/b/ f",
        "sed -i.bak s/a/b/ f",
        "sed -n p f",
        "sed -n 1p f",
        "sed -e 'w out' f",
        "awk '{print}' f",
        "awk 'BEGIN{system(\"x\")}'",
        "xargs rm",
        "xargs ls",
        "ls | xargs cat",
        "env ls",
        "env",
        "sudo ls",
        "doas ls",
        "less f",
        "more f",
        "view f",
        "vim f",
        "python3 -c 'print(1)'",
        "python x.py",
        "perl -e 'print 1'",
        "node -e 1",
        "ruby -e 1",
        "bash -c ls",
        "sh -c ls",
        "zsh -c ls",
        "eval ls",
        "exec ls",
        "source x",
        ". x",
        "command ls",
        "builtin ls",
        "nice ls",
        "nohup ls",
        "time ls",
        "timeout 5 ls",
        "watch ls",
        "rm -rf x",
        "rm f",
        "touch f",
        "mv a b",
        "cp a b",
        "mkdir d",
        "chmod +x f",
        "ln -s a b",
        "curl https://example.com",
        "wget https://example.com",
        "npm test",
        "cargo build",
        "make",
        "docker ps",
        "kubectl get pods",
        "ssh host ls",
        "open .",
        "xdg-open .",
        "ps aux",
        "kill 1",
        "dd if=a of=b",
        "install a b",
        "tar tf a.tar",
        "unzip -l a.zip",
        "zcat a.gz",
        "strings a.bin",
        "hexdump a.bin",
        "od a.bin",
        "ls-extra",
        "lss",
        "/bin/ls",
        "/usr/bin/git status",
        "./ls",
        "ls/",
        "\\ls",
        "./git status",
        "GIT status",
        "LS",
        "Ls",
    ]);
}

// --- options that make a read-only program write or run something -----------------------------------------

#[test]
fn find_options_that_write_or_run_are_not_proven() {
    assert_none_proven(&[
        "find . -exec rm '{}' ';'",
        "find . -exec rm '{}' +",
        "find . -exec cat '{}' ';'",
        "find . -name x -exec cat '{}' +",
        "find . -execdir rm '{}' ';'",
        "find . -ok rm '{}' ';'",
        "find . -okdir rm '{}' ';'",
        "find . -delete",
        "find . -name x -delete",
        "find . -type f -delete -print",
        "find . -fprint out.txt",
        "find . -fprint0 out.txt",
        "find . -fprintf out.txt '%p'",
        "find . -fls out.txt",
        "find . -name x -fprint /dev/null",
        "find . -exec",
        "find -exec ls ';'",
        "find . -files0-from list",
        "find -L . -name x",
        "find -H . -name x",
        "find . -follow",
        "find . -newerXY x",
        "find . -foo",
        "find . -name",
        "find . -name x -",
        "find .. -name x",
        "find ../x -name x",
        "find . -execute x",
        "find . -EXEC x ';'",
        "find . -exe x",
        "find . -del",
        "find . -fprin out",
        "find . -ls -exec x ';'",
        // a write predicate with no file, and the file given as a stray word
        "find . -fprint",
        "find . -fprint0",
        "find . -fls",
        "find . -fprintf",
        "find . -fprintf '%p'",
        "find . -name x -fprint",
        "find . -fprint /dev/null",
        "find . -fprint0 /dev/null",
        "find . -name x -o -exec x ';'",
        "find . -name x -a -delete",
    ]);
}

#[test]
fn options_that_write_run_or_follow_are_not_proven_per_program() {
    assert_none_proven(&[
        // sort
        "sort -o out a",
        "sort -oout a",
        "sort --output=out a",
        "sort --output out a",
        "sort --out=out a",
        "sort -uo out a",
        "sort -ou out a",
        "sort --compress-program=gzip a",
        "sort --compress-program gzip a",
        "sort -T /tmp a",
        "sort -T/tmp a",
        "sort --temporary-directory=/tmp a",
        "sort --files0-from=list",
        "sort -R a",
        // tail
        "tail -f log",
        "tail -F log",
        "tail --follow log",
        "tail --follow=name log",
        "tail -fn 5 log",
        "tail --retry log",
        "tail -s 1 log",
        // uniq
        "uniq a b",
        "uniq -c a b",
        "uniq a out.txt",
        "uniq - out.txt",
        // rg
        "rg --pre cat x",
        "rg --pre=cat x",
        "rg --pre-glob '*.gz' x",
        "rg -z x",
        "rg --search-zip x",
        "rg -L x",
        "rg --follow x",
        "rg --hostname-bin=sh x",
        "rg --no-config --pre sh x",
        // grep
        "grep -R x .",
        "grep --dereference-recursive x .",
        "grep --exclude-from=list x .",
        "grep --foo x",
        "grep",
        // ls, du, tree, stat, file, diff
        "ls -L",
        "ls --dereference",
        "ls -R -L",
        "ls -w 5",
        "ls --time-style",
        "du -L .",
        "du --files0-from=list",
        "tree -o out src",
        "tree -R",
        "tree -l",
        "tree -H base",
        "tree --fromfile list",
        "stat -f x",
        "stat -t x",
        "stat -L x",
        "stat --dereference x",
        "file -z a",
        "file -C",
        "file -m magic a",
        "file -f list",
        "file -L a",
        "diff -r a b",
        "diff --recursive a b",
        "diff --to-file=x a",
        "diff --from-file=x a",
        "diff -X ex a b",
        "diff a",
        "jq -f prog.jq a.json",
        "jq --from-file prog.jq a.json",
        "jq -L lib '.' a.json",
        "jq --slurpfile k f '.' a.json",
        "jq --rawfile k f '.' a.json",
        "jq --arg k v '.' a.json",
        // jq can print the environment, and load modules from the file system
        "jq -n env",
        "jq -n 'env'",
        "jq -n 'env.HOME'",
        "jq -n '$ENV'",
        "jq '$ENV.PATH' a.json",
        "jq -n '$ENV | keys'",
        "jq -n '$env'",
        "jq '.env' a.json",
        "jq 'import \"a\" as a; .' x",
        "jq 'include \"a\"; .' x",
        "jq 'modulemeta' x",
        "jq",
        // cut, tr, head, wc, cat
        "cut -f1 --foo a",
        "tr",
        "tr a b c",
        "tr -o a b",
        "wc --files0-from=list",
        "wc --files0-from list",
        "head -f a",
        "head -n",
        "cat -x a",
        "cat --foo a",
        // a lone dash is standard input, not a file the path check can see
        "cat -",
        "grep x -",
        // pwd, which, basename, dirname, realpath
        "pwd x",
        "pwd -x",
        "which -x cargo",
        "basename",
        "dirname",
        "realpath -x a",
    ]);
}

#[test]
fn git_subcommands_and_options_that_write_or_run_are_not_proven() {
    assert_none_proven(&[
        "git",
        "git --version",
        "git help",
        "git status; git push",
        "git add .",
        "git add -A",
        "git commit -m x",
        "git commit --amend",
        "git push",
        "git pull",
        "git fetch",
        "git clone https://example.com/r.git",
        "git checkout main",
        "git checkout -- f",
        "git switch main",
        "git restore f",
        "git reset --hard",
        "git reset",
        "git clean -fd",
        "git rm f",
        "git mv a b",
        "git stash",
        "git stash list",
        "git merge x",
        "git rebase x",
        "git cherry-pick x",
        "git revert x",
        "git tag v1",
        "git tag",
        "git init",
        "git config user.name x",
        "git config --list",
        "git apply p.diff",
        "git am p",
        "git submodule update",
        "git submodule foreach ls",
        "git worktree add ../w",
        "git worktree list",
        "git gc",
        "git prune",
        "git update-index --refresh",
        "git update-ref -d refs/heads/x",
        "git cat-file -p HEAD",
        "git grep x",
        "git fsck",
        "git bisect run ls",
        "git difftool",
        "git mergetool",
        "git instaweb",
        "git daemon",
        "git archive HEAD",
        "git bundle create b HEAD",
        "git format-patch -1",
        "git notes add -m x",
        "git reflog",
        "git reflog expire --all",
        "git shortlog",
        "git whatchanged",
        "git diff-tree HEAD",
        "git for-each-ref",
        "git ls-remote",
        "git ls-tree HEAD",
        "git status-extra",
        "git STATUS",
        "git Status",
        "git s",
        "git st",
        "git co",
        "git alias",
        // branch and remote only list
        "git branch newbranch",
        "git branch newbranch HEAD",
        "git branch -d x",
        "git branch -D x",
        "git branch -m a b",
        "git branch -M a b",
        "git branch -c a b",
        "git branch -C a b",
        "git branch -f x HEAD",
        // the same options with no operand: they still write or delete when given what they need
        "git branch -d",
        "git branch -D",
        "git branch -m",
        "git branch -M",
        "git branch -c",
        "git branch -C",
        "git branch -u",
        "git branch -f",
        "git branch -t",
        "git branch -l",
        "git branch --delete",
        "git branch --move",
        "git branch --copy",
        "git branch --force",
        "git branch --track",
        "git branch --set-upstream-to",
        "git branch -vd",
        "git branch -u origin/x",
        "git branch --set-upstream-to=origin/x",
        "git branch --unset-upstream",
        "git branch --edit-description",
        "git branch --delete x",
        "git branch --move a b",
        "git branch --copy a b",
        "git branch --force x",
        "git branch -l x",
        "git branch --list x",
        "git branch -t x",
        "git branch --track x",
        "git remote add o https://example.com/r.git",
        "git remote remove o",
        "git remote rm o",
        "git remote rename a b",
        "git remote set-url o u",
        "git remote prune o",
        "git remote update",
        "git remote show origin",
        "git remote get-url origin",
        "git remote -v add o u",
        "git remote -v origin",
        // global options that load config, run programs or move the repository
        "git -c core.pager=x log",
        "git -c alias.x=status x",
        "git -ccore.pager=x log",
        "git --config-env=core.pager=X log",
        "git --config-env core.pager=X log",
        "git --exec-path=/x log",
        "git --exec-path log",
        "git --git-dir=/x log",
        "git --git-dir /x log",
        "git --work-tree=/x status",
        "git --work-tree /x status",
        "git -C /x log",
        "git -C. log",
        "git -C ./x log",
        "git --namespace=x log",
        "git --super-prefix=x log",
        "git --paginate log",
        "git -p log",
        "git -P log",
        "git --bare log",
        "git --no-replace-objects log",
        "git --literal-pathspecs log",
        "git --html-path",
        // options after the subcommand
        "git log --output=out",
        "git log --output out",
        "git log --out=out",
        "git diff --output=out",
        "git diff --output out",
        "git show --output=out",
        "git diff --ext-diff",
        "git diff --ext-diff --stat",
        "git log --ext-diff -p",
        "git show --ext-diff",
        "git diff --textconv",
        "git log --show-signature",
        "git log --exec x",
        "git log -c",
        "git log --cc",
        "git diff --no-index a b",
        "git diff --no-index -- a b",
        "git blame --contents f x",
        "git blame -S revs f",
        "git blame --ignore-revs-file=r f",
        "git status --foo",
        "git log --foo",
        "git ls-files -X file",
        "git ls-files --exclude-from=file",
        "git ls-files --exclude-per-directory=x",
        "git rev-parse --foo",
        "git describe --foo",
        // a word that can name a path outside the work tree, or a blob
        "git show HEAD:secret.txt",
        "git show :secret.txt",
        "git show :0:secret.txt",
        "git log -p HEAD:f",
        "git diff HEAD:a HEAD:b",
        "git show 'HEAD:f'",
        "git ls-files ':(top)x'",
        "git ls-files :/x",
    ]);
}

// --- environment assignments, aliases and variables -------------------------------------------------------

#[test]
fn an_environment_assignment_is_not_proven() {
    assert_none_proven(&[
        "FOO=bar ls",
        "FOO= ls",
        "FOO=bar git status",
        "GIT_PAGER=x git log",
        "GIT_EXTERNAL_DIFF=x git diff",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.pager GIT_CONFIG_VALUE_0=x git log",
        "GIT_DIR=/x git log",
        "GIT_WORK_TREE=/x git status",
        "GIT_SSH_COMMAND=x git status",
        "PAGER=x git log",
        "LESS=x git log",
        "EDITOR=x git log",
        "BASH_ENV=x ls",
        "ENV=x ls",
        "LD_PRELOAD=x ls",
        "DYLD_INSERT_LIBRARIES=x ls",
        "PATH=/tmp ls",
        "PATH=. ls",
        "IFS=x ls",
        "HOME=/x ls",
        "TMPDIR=/x sort a",
        "CDPATH=/x ls",
        "CDPATH=/x cd y && ls",
        "A=b",
        "A=b;ls",
        "A=b && ls",
        "ls && A=b ls",
        "ls | A=b cat",
        "a+=b ls",
        "a[0]=b ls",
        "A='b c' ls",
        "A=\"b\" ls",
        "A=$(ls) ls",
        "A=`ls` ls",
        "export A=b",
        "export A=b && ls",
        "unset A",
        "set -x",
        "alias ls=rm",
        "declare A=b",
        "local A=b",
        "readonly A=b",
        "=ls",
        "=ls -la",
        "ls =ls",
        "ls =(ls)",
        "cat =git",
    ]);
}

// --- cd ---------------------------------------------------------------------------------------------------

#[test]
fn a_cd_that_is_not_a_literal_leading_one_is_not_proven() {
    assert_none_proven(&[
        "cd",
        "cd -",
        "cd ~",
        "cd ~/x",
        "cd ~user",
        "cd ~+",
        "cd ~-",
        "cd $X",
        "cd \"$X\"",
        "cd ${X}",
        "cd $HOME",
        "cd $PWD/x",
        "cd /a$X",
        "cd ..",
        "cd ../x",
        "cd ./..",
        "cd /a/..",
        "cd /a/../b",
        "cd a/../b",
        "cd src",
        "cd src/lib",
        "cd '/a b'",
        "cd \"/a b\"",
        "cd '$X'",
        "cd /a*",
        "cd /a?",
        "cd /a[b]",
        "cd /{a,b}",
        "cd -P /a",
        "cd -L /a",
        "cd -- /a",
        "cd /a /b",
        "cd -e /a",
        "cd -@ /a",
        "cd /a > f",
        "cd /a 2>/dev/null",
        // a redirect on the `cd` can write a file as well as one on any other command
        "cd /a > out.txt && ls",
        "cd /a >out.txt && ls",
        "cd /a >> out.txt && ls",
        "cd /a 2>/dev/null && ls",
        "cd /a >/dev/null && ls",
        "cd /a 2>&1 && ls",
        "cd /a < f && ls",
        "cd /a <<< x && ls",
        "cd /a && ",
        "cd /a &&",
        "cd /a",
        "cd /a;",
        "cd /a; ls",
        "cd /a ; ls",
        "cd /a || ls",
        "cd /a & ls",
        "cd /a\nls",
        "cd /a | ls",
        "cd /a |& ls",
        "ls | cd /a",
        "ls && cd /a",
        "ls && cd /a && ls",
        "ls ; cd /a",
        "cd /a && ls && cd /b && ls",
        "cd /a && ls; cd /b",
        "(cd /a && ls)",
        "(cd /a) && ls",
        "{ cd /a; ls; }",
        "cd /a && (ls)",
        "pushd /a && ls",
        "popd && ls",
        "chdir /a && ls",
        "builtin cd /a && ls",
        "command cd /a && ls",
        "\\cd /a && ls",
        "cd /a && cd",
        "cd /a && cd - && ls",
        "cd /a && cd ~ && ls",
        "cd /a && cd $X && ls",
        "cd /a && cd .. && ls",
        "cd ./ && cd .. && ls",
        // a relative target with no `./` can be a CDPATH lookup
        "cd a && ls",
        "cd a/b && ls",
        "cd .hidden && ls",
        "cd -x && ls",
        "cd '' && ls",
        "cd \"\" && ls",
        "cd '/a' '/b' && ls",
    ]);
}

#[test]
fn literal_cd_target_accepts_only_an_absolute_or_dot_slash_path() {
    for word in [
        "/",
        "/a",
        "/a/b",
        "/a/b/",
        "/usr/local/lib",
        "/srv/app/my-proj_1.2",
        "/a,b",
        "/a:b",
        "/a+b",
        "/a@b",
        "/a%b",
        "/a=b",
        "/.hidden",
        "/a/.b",
        "/a/...",
        "./a",
        "./a/b",
        "./",
        "./.hidden",
        "./a-b_c.d",
    ] {
        assert_eq!(literal_cd_target(word), Some(word), "{word}");
    }
    for word in [
        "",
        ".",
        "..",
        "./..",
        "../a",
        "a",
        "a/b",
        "-",
        "--",
        "-a",
        "-P",
        "~",
        "~/a",
        "~user",
        "~+",
        "~-",
        "/a/..",
        "/a/../b",
        "/../a",
        "/..",
        "./a/..",
        "./a/../b",
        "//a/..//b",
        "$X",
        "/a$X",
        "/${X}",
        "/a`b`",
        "/a b",
        "/a\tb",
        "/a\nb",
        "/a\rb",
        "/a\0b",
        "/a*",
        "/a?",
        "/a[b]",
        "/a{b,c}",
        "/a'b",
        "/a\"b",
        "/a\\b",
        "/a;b",
        "/a&b",
        "/a|b",
        "/a>b",
        "/a<b",
        "/a(b)",
        "/a#b",
        "/a!b",
        "/a^b",
        "/a~b",
        "=/a",
        "/é",
        "/a\u{a0}b",
        "/a\u{85}b",
        "/a\u{2028}b",
        ".hidden",
        ".a",
    ] {
        assert_eq!(literal_cd_target(word), None, "{word:?}");
    }
    assert_eq!(literal_cd_target(&format!("/{}", "a".repeat(5000))), None);
}

#[test]
fn head_tail_count_accepts_only_a_literal_count() {
    for (args, count) in [
        (&["-n", "5"][..], 5),
        (&["-n", "0"], 0),
        (&["-n", "100"], 100),
        (&["-n5"], 5),
        (&["-n050"], 50),
        (&["-20"], 20),
        (&["-n", "999999999"], 999_999_999),
    ] {
        let args: Vec<String> = strs(args);
        assert_eq!(head_tail_count(&args), Some(count), "{args:?}");
    }
    for args in [
        &[][..],
        &["-n"],
        &["-n", ""],
        &["-n", "-5"],
        &["-n", "+5"],
        &["-n", "5k"],
        &["-n", "0x5"],
        &["-n", "5.0"],
        &["-n", " 5"],
        &["-n", "5 "],
        &["-n", "$N"],
        &["-n", "\"$N\""],
        &["-n", "$(echo 5)"],
        &["-n", "`echo 5`"],
        &["-n", "5", "file"],
        &["-n", "5", "-n", "6"],
        &["-n", "5", "-q"],
        &["-n5", "file"],
        &["-n5", "-n6"],
        &["-nx"],
        &["-n$N"],
        &["-n-5"],
        &["-n+5"],
        &["-n", "५"],
        &["-n", "٥"],
        &["-n", "5\n"],
        &["-n", "99999999999999999999999"],
        &["-c", "5"],
        &["-c5"],
        &["-f"],
        &["-F"],
        &["--follow"],
        &["--lines", "5"],
        &["--lines=5"],
        &["-q"],
        &["-v"],
        &["-z", "-n", "5"],
        &["5"],
        &["--", "5"],
        &["-"],
        &["--"],
        &["file"],
        &["-n", "5", "--"],
        &["-n 5"],
        &["-0x5"],
        &["-5k"],
        &["-n", "5", ">", "x"],
        &["-n", "5", "|", "tee", "x"],
    ] {
        let args: Vec<String> = strs(args);
        assert_eq!(head_tail_count(&args), None, "{args:?}");
    }
}

// --- chains: only `&&` and `|` ----------------------------------------------------------------------------

#[test]
fn a_chain_other_than_and_or_pipe_is_not_proven() {
    assert_none_proven(&[
        "ls; ls",
        "ls ;",
        "ls ;ls",
        "ls;ls",
        ";ls",
        "ls ;; ls",
        "ls || ls",
        "ls||ls",
        "ls || true",
        "ls && ls || ls",
        "ls & ls",
        "ls &",
        "ls&",
        "ls &\nls",
        "ls\nls",
        "ls\n",
        "\nls",
        "ls\r\nls",
        "ls\rls",
        "ls |& cat",
        "ls |&cat",
        "ls | | cat",
        "ls || | cat",
        "| ls",
        "|| ls",
        "&& ls",
        "& ls",
        "ls &&",
        "ls && &&",
        "ls &&&& ls",
        "ls &&& ls",
        "ls |",
        "ls | ",
        "(ls)",
        "( ls )",
        "(ls) && ls",
        "ls && (ls)",
        "ls | (cat)",
        "{ ls; }",
        "{ ls ; } | cat",
        "! ls",
        "!ls",
        "ls !",
        "ls ! x",
        "if true; then ls; fi",
        "if ls; then ls; fi",
        "for i in 1 2; do ls; done",
        "while ls; do ls; done",
        "until ls; do ls; done",
        "case x in x) ls;; esac",
        "select i in 1; do ls; done",
        "function f { ls; }",
        "f() { ls; }; f",
        "f() { ls; }",
        "coproc ls",
        "time ls",
        "ls #comment",
        "ls # comment",
        "ls #",
        "# ls",
        "#ls",
        "ls a#b",
        "ls \\\nls",
        "ls \\",
        "ls\\ x",
        "ls a\\ b",
        "ls \"a\\\"b\"",
        "ls \"a\\\\b\"",
    ]);
}

// --- tilde, brace, glob, `..` and zsh glob operators ------------------------------------------------------

#[test]
fn a_word_the_shell_expands_to_something_unpredictable_is_not_proven() {
    assert_none_proven(&[
        // tilde
        "ls ~",
        "ls ~/x",
        "ls ~root",
        "ls ~+",
        "ls ~-",
        "ls a~b",
        "ls a/~",
        "ls --color=~",
        "ls x=~/y",
        "cat ~/.ssh/id_rsa",
        "cat -- ~/x",
        // brace
        "ls {a,b}",
        "ls a{b,c}",
        "ls {a,b}c",
        "ls {1..3}",
        "ls {a..c}",
        "ls {a}",
        "ls {",
        "ls }",
        "ls a{",
        "ls a}",
        "ls {a,b}/{c,d}",
        "ls ${a,b}",
        "cat {a,b}",
        "ls {,}",
        "ls { a }",
        // a glob that can match `..`
        "ls .*",
        "ls .?",
        "ls .[a]",
        "ls .[.]",
        "ls .[!a]",
        "ls ./.*",
        "ls ./.?",
        "ls src/.*",
        "ls src/.?",
        "ls src/.[a]",
        "ls /tmp/.*",
        "ls .*/x",
        "ls a/.*/b",
        "ls '.'*",
        "ls \".\"*",
        "ls '.'?",
        "ls \".\"[.]",
        "ls [.]*",
        "ls [.][.]",
        "ls [.]?",
        "ls [!a]*/.*",
        "cat .*",
        "grep x .*",
        "rg x .*",
        "wc -l .*",
        "du -sh .*",
        "ls -d .*",
        "find .* -name x",
        // a `..` component
        "ls ..",
        "ls ../x",
        "ls ../../x",
        "ls x/..",
        "ls x/../y",
        "ls ./..",
        "ls ./../x",
        "ls /a/../b",
        "ls '..'",
        "ls \"..\"",
        "ls '../x'",
        "ls \"x/../y\"",
        "ls ..'/x'",
        "ls '..'/x",
        "ls .''.",
        "cat ../k",
        "cat ../../etc/passwd",
        "cat a/../../b",
        "cat < ../x",
        "grep x ../src",
        "rg x ..",
        "find .. -name x",
        "find . -newer ../x",
        "head -n 5 ../x",
        "git log -- ..",
        "git log -- ../x",
        "git diff ../x",
        "git diff -- a/../b",
        "ls --color=../x",
        "ls --foo=../x",
        "grep --include=../x y .",
        "grep -e ../x y",
        "grep ../x y",
        "rg ../x .",
        "jq '..' a.json",
        "jq .. a.json",
        // zsh glob operators and expansions
        "ls ^x",
        "ls a^b",
        "ls x#y",
        "ls x##y",
        "ls x~y",
        "ls *~x",
        "ls *(e:'x':)",
        "ls *(.)",
        "ls *(/)",
        "ls *(N)",
        "ls **(e:x:)",
        "ls (a|b)",
        "ls a(b|c)",
        "ls ?(a)",
        "ls *(a)",
        "ls +(a)",
        "ls @(a)",
        "ls !(a)",
        "ls <1-3>",
        "ls <->",
        "ls =ls",
        "ls =(ls)",
        "ls *.rs(e:x:)",
        "ls ^*",
        // other metacharacters
        "ls !x",
        "ls x!",
        "ls !!",
        "ls !$",
        "ls ^x^y",
        "ls a;b",
        "ls a|b|",
        "ls a&b",
        "ls a>b",
        "ls (a)",
        "ls )",
        "ls (",
        "ls `",
        "ls \\",
        "ls \\n",
        "ls \\ ",
    ]);
}

/// A glob can expand to a file name that starts with `-`, and the program then reads it as an option
/// (`sort *` with a file called `--output=x`). Only a glob with a literal first character other than `-`, in
/// the place of a path, or after `--`, is proven.
#[test]
fn an_unquoted_glob_that_can_become_an_option_or_a_value_is_not_proven() {
    assert_none_proven(&[
        "ls *",
        "ls *.rs",
        "ls ?x",
        "ls [ab]*",
        "ls -*",
        "ls -l*",
        "ls -la *.rs",
        "wc -l *",
        "wc -l *.txt",
        "cat *",
        "sort *",
        "sort -u *.txt",
        "grep foo *",
        "grep -rn foo *.rs",
        "rg foo *",
        "git log *",
        "git diff *.rs",
        "find * -name x",
        "find . -name *.rs",
        "find . -type *",
        "head -n * a",
        "head -n 5 *",
        "grep --include *.c foo .",
        "grep *.c a",
        "grep -e * a",
        "rg -g *.rs foo",
        "jq * a.json",
        "cut -d * a",
        "tr * x",
        "basename *",
        "which *",
        "ls *.rs | head -n 3",
        "cd /a && ls *",
    ]);
    // a literal first character other than `-` keeps every expansion an operand
    assert_eq!(paths_of("ls src/*.rs"), strs(&["src/*.rs"]));
    assert_eq!(paths_of("ls ./*.rs"), strs(&["./*.rs"]));
    assert_eq!(paths_of("ls /tmp/x*"), strs(&["/tmp/x*"]));
    assert_eq!(paths_of("cat a?.txt"), strs(&["a?.txt"]));
    assert_eq!(paths_of("ls ./[ab]*"), strs(&["./[ab]*"]));
    assert_eq!(paths_of("grep foo src/*.rs"), strs(&["src/*.rs"]));
    assert_eq!(paths_of("git log -- *.rs"), strs(&["*.rs"]));
    assert_eq!(paths_of("cat -- *"), strs(&["*"]));
    assert_eq!(paths_of("ls .a*"), strs(&[".a*"]));
    assert_eq!(paths_of("ls ./...*"), strs(&["./...*"]));
    // a quoted glob is a literal word
    assert_eq!(paths_of("ls '*'"), strs(&["*"]));
    assert_eq!(paths_of("grep '*' a"), strs(&["a"]));
    assert_eq!(paths_of("find . -name '*.rs'"), strs(&["."]));
    // after `--` a glob cannot become an option, but it still may not match `..`
    assert_none_proven(&[
        "cat -- .*",
        "cat -- ..*",
        "cat -- ./.*",
        "cat -- [.]*",
        "cat -- ./[.]*",
        "cat -- ./[a-c]*",
        "cat -- ./[[:punct:]]*",
        "cat -- ./[[=.=]]*",
        "ls ./[.]*",
        "ls ./[a-c]*",
        "ls ./[+-/]*",
    ]);
    // the glob rules `..` and `.*` still apply
    assert_none_proven(&[
        "ls ..*", "ls ./..*", "ls .*", "ls ./.?", "ls .[a]", "ls [.]*",
    ]);
}

#[test]
fn quoting_that_is_unbalanced_or_unusual_is_not_proven() {
    assert_none_proven(&[
        "ls 'abc",
        "ls \"abc",
        "ls 'a'\"b",
        "ls \"a'b",
        "ls 'a\"b",
        "ls '",
        "ls \"",
        "ls ''\"",
        "ls \"'",
        "ls '\\''",
        "ls $'a'",
        "ls $'\\x41'",
        "ls $'\\u0041'",
        "ls $'\\n'",
        "ls $'a\\'b'",
        "ls $'\\''",
        "ls $\"a\"",
        "ls $\"$X\"",
        "ls \"a\\$b\"",
        "ls \"a\\`b\"",
        "ls \"!x\"",
        "ls \"a!\"",
        "ls \"a!b\"",
        "ls \"$\"",
        "ls \"`\"",
        "ls \"\\\"",
        "ls \"\\n\"",
        "ls '\n'",
        "ls \"\n\"",
        "ls 'a\nb'",
        "ls \"a\nb\"",
        "ls 'a\rb'",
        "ls 'a\0b'",
        "ls 'a\u{1}b'",
        "ls 'a\u{7f}b'",
        "ls '\u{1b}[31m'",
    ]);
}

#[test]
fn control_characters_and_unquoted_non_ascii_are_not_proven() {
    assert_none_proven(&[
        "ls\0",
        "ls\0 x",
        "ls \0x",
        "ls\u{b}x",
        "ls\u{c}x",
        "ls\u{1}",
        "ls\u{1b}[31m",
        "ls\u{7f}",
        "ls\rx",
        "ls\nx",
        "ls é",
        "ls \u{a0}x",
        "ls\u{a0}x",
        "ls \u{85}x",
        "ls \u{2028}x",
        "ls \u{2029}x",
        "ls \u{3000}x",
        "ls \u{feff}x",
        "ls \u{200b}x",
        "ls \u{202e}x",
        "ls x\u{301}",
        "ls \u{ff1b} ls",
        "ls \u{ff06}\u{ff06} ls",
        "ls \u{1f600}",
    ]);
    // the same text quoted is a plain word
    proven("ls 'é'");
    proven("ls \"é\"");
}

#[test]
fn a_command_over_the_length_limit_is_not_proven() {
    let long = format!("ls {}", "a".repeat(5000));
    assert_none_proven(&[&long]);
    let many = format!("ls {}", "a ".repeat(3000));
    assert_none_proven(&[&many]);
    proven(&format!("ls {}", "a".repeat(100)));
}

// --- the shared parser must read the line the same way ---------------------------------------------------

#[test]
fn every_proven_command_is_read_by_analyze_as_plain_simple_commands() {
    for command in [
        "ls",
        "ls -la src",
        "cat a.txt | grep foo | sort | uniq -c",
        "cd /w && ls src && git status",
        "cd /a && cd ./b && cat k.txt | head -n 3",
        "grep -rn 'a|b' src",
        "jq -r '.[] | .id' a.json",
        "ls 2>&1 | head -n 5",
        "cat < a.txt",
        "git log --pretty=format:%h -- src",
    ] {
        let analysis = analyze(command, HOME);
        assert!(!analysis.incomplete, "{command}");
        assert!(analysis.alternates.is_empty(), "{command}");
        for entry in &analysis.entries {
            assert!(entry.opaque.is_empty(), "{command}: {entry:?}");
            assert!(
                READONLY_PROGRAMS.contains(&entry.prog.as_str()) || entry.prog == "cd",
                "{command}: {entry:?}"
            );
        }
        proven(command);
    }
}

// --- the runtime is macOS BSD tools under zsh (review round 1) ----------------------------------------------

#[test]
fn a_word_with_a_dash_after_the_first_operand_is_not_proven() {
    // BSD getopt stops at the first operand, GNU getopt_long permutes: the word is an option on one and a
    // file name on the other, so neither reading is proven. `uniq a -c` writes the file `-c` on macOS.
    assert_none_proven(&[
        "uniq payload.txt -c",
        "head notes.md -n 5",
        "cat notes.md -n",
        "tail log.txt -n 5",
        "wc a.txt -l",
        "ls dir -l",
        "ls src -la",
        "du . -h",
        "stat f -c %s",
        "realpath p -e",
        "file a.png -b",
        "sort a.txt -r",
        "cut a.csv -d, -f1",
        "diff a b -u",
        "grep foo a.txt -n",
        "grep foo -r src",
        "grep -e foo a.txt -n",
        "rg foo src -n",
        "rg foo -n",
        "jq . a.json -c",
        "jq . -c a.json",
        "git log main -n 5",
        // git reads `--` the same on every platform, but the rule is one for every program
        "git diff main -- src",
        "git log main -- a.rs",
        "git diff a.rs --stat",
        "git show HEAD -s",
        "git ls-files src -o",
        // `--` after an operand is a file name on BSD and the end of the options on GNU
        "cat a -- b",
        "ls a -- b",
        "cat a --",
        // behind a pipe, behind a cd, after a redirect: the rule is per command
        "ls | head a.txt -n 5",
        "cd /w && cat notes.md -n",
        "cat a.txt -n < b.txt",
    ]);
    // options first, operands after, and everything after `--`, still pass
    for command in [
        "cat -n a.txt",
        "cat -n -- a.txt",
        "cat -- -n",
        "cat -- a -b",
        "cat a.txt b.txt",
        "head -n 5 notes.md",
        "uniq -c payload.txt",
        "grep -rn foo src",
        "grep -e foo -r src",
        "grep -n -- foo -x",
        "rg -n foo src",
        "git log -n 5 -- src -x",
        "git diff --stat a.rs b.rs",
        "diff -u a b",
        "ls -la src tests",
        "find . -name '*.rs' -type f",
    ] {
        proven(command);
    }
    // an option value may start with `-`: it is the value, not a word after an operand
    proven("head -n -5 notes.md");
    proven("grep -e -foo a.txt");
}

#[test]
fn a_zsh_numeric_glob_is_not_a_pair_of_redirects() {
    // zsh reads `<1-5>`, `<-9>` and `<->` as one glob word; bash reads two redirects. Either way the line is
    // not one the classifier can describe.
    assert_none_proven(&[
        "cat <1-5>/dev/null",
        "cat <1-5>&2 x",
        "cat <-9>/dev/null",
        "cat <->/dev/null",
        "cat <0-9>&2",
        "cat <1->/dev/null",
        "cat <-5>x",
        "cat a<1-5>b",
        "cat 'a'<1-5>",
        "cat 2<1-5>",
        "ls <1-5>",
    ]);
    // a `<` and a `>` that are no numeric glob are plain redirects
    proven("cat <a.txt");
    proven("cat < a.txt");
    proven("cat <a.txt >/dev/null");
    // a quoted `<1-5>` is text
    proven("grep '<1-5>' a.txt");
}

#[test]
fn a_word_that_zsh_expands_as_a_command_path_is_not_proven() {
    // EQUALS: a word that opens with `=` is the path of that command, however the `=` is spelled
    assert_none_proven(&[
        "cat =ls",
        "cat ''=ls",
        "cat \"\"=ls",
        "cat '='ls",
        "cat \"=\"ls",
        "cat ''''=ls",
        "cat ''=",
        "ls -- ''=ls",
        // MAGIC_EQUAL_SUBST: `name=` and `:` open a position where `=cmd` is expanded again
        "cat a==ls",
        "cat a:=ls",
        "cat --x==ls",
        "grep --color==ls x",
    ]);
    // an `=` inside a word is text
    proven("grep --color=auto foo a.txt");
    proven("cat a=b");
    proven("cat 'a'=b");
    proven("git log --pretty=format:%h");
    proven("git log --since=2.weeks");
}

#[test]
fn git_commands_say_that_git_runs() {
    for command in [
        "git status",
        "git log -p",
        "git diff",
        "git show HEAD",
        "git blame -L 1,10 a.rs",
        "git ls-files",
        "git rev-parse HEAD",
        "git describe --tags",
        "git branch",
        "git remote -v",
        "cd ./sub && git status",
        "ls | git log",
        "git --no-pager log -3 | head -n 5",
    ] {
        assert!(proven_full(command).2, "{command}");
    }
    for command in [
        "ls",
        "cat a.txt",
        "cd /w && rg foo",
        "pwd",
        "grep git a.txt",
    ] {
        assert!(!proven_full(command).2, "{command}");
    }
}

#[test]
fn git_options_that_refresh_the_index_are_not_proven() {
    // `describe --dirty` and `--broken` refresh `.git/index`: a write, and a config-driven run
    assert_none_proven(&[
        "git describe --dirty",
        "git describe --dirty=-x",
        "git describe --broken",
        "git describe --tags --dirty",
    ]);
    proven("git describe --tags --abbrev=0");
}

#[test]
fn each_path_says_whether_the_command_reads_everything_under_it() {
    let table: &[(&str, &[(&str, bool)])] = &[
        // rg always walks directories
        ("rg foo", &[(".", true)]),
        ("rg foo src", &[("src", true)]),
        ("rg --files", &[(".", true)]),
        ("rg -n foo a.txt b.txt", &[("a.txt", true), ("b.txt", true)]),
        // grep only with -r, --recursive or -d recurse
        ("grep foo a.txt", &[("a.txt", false)]),
        ("grep -rn foo src", &[("src", true)]),
        ("grep -r foo", &[(".", true)]),
        ("grep --recursive foo src", &[("src", true)]),
        ("grep -nr foo src", &[("src", true)]),
        ("grep -d recurse foo src", &[("src", true)]),
        ("grep -drecurse foo src", &[("src", true)]),
        ("grep --directories=recurse foo src", &[("src", true)]),
        ("grep -d skip foo src", &[("src", false)]),
        ("egrep -r a src", &[("src", true)]),
        ("fgrep -r a src", &[("src", true)]),
        ("grep foo", &[(".", false)]),
        ("grep -f pats.txt a", &[("pats.txt", false), ("a", false)]),
        // find, du, tree always walk
        ("find . -name x", &[(".", true)]),
        ("find src tests -type f", &[("src", true), ("tests", true)]),
        ("find -name x", &[(".", true)]),
        (
            "find src -newer Cargo.lock",
            &[("src", true), ("Cargo.lock", false)],
        ),
        ("du -sh .", &[(".", true)]),
        ("du -sh", &[(".", true)]),
        ("tree -L 2 src", &[("src", true)]),
        ("tree", &[(".", true)]),
        // ls lists a directory unless -R
        ("ls", &[(".", false)]),
        ("ls -la src", &[("src", false)]),
        ("ls -R", &[(".", true)]),
        ("ls -lR src", &[("src", true)]),
        ("ls --recursive src", &[("src", true)]),
        // plain file readers
        ("cat a.txt", &[("a.txt", false)]),
        ("cat < a.txt", &[("a.txt", false)]),
        // two directories are compared file by file: every file they share is read
        ("diff a b", &[("a", true), ("b", true)]),
        ("diff -u ./a ./b", &[("./a", true), ("./b", true)]),
        // a `**` glob walks the tree under it, whatever the program
        ("cat src/**/x.rs", &[("src/**/x.rs", true)]),
        ("ls src/**", &[("src/**", true)]),
        ("grep foo src/***/x", &[("src/***/x", true)]),
        ("cat src/*/x.rs", &[("src/*/x.rs", false)]),
        ("jq . a.json", &[("a.json", false)]),
        ("wc -l a b", &[("a", false), ("b", false)]),
        // a git path names a directory pathspec as often as a file
        ("git log -- src", &[("src", true)]),
        ("git diff -- a.rs", &[("a.rs", true)]),
        ("git diff main", &[("main", true)]),
        // the recursive flag belongs to the command, not the line
        ("ls -R | head -n 3", &[(".", true)]),
        (
            "cat a.txt | grep -r foo src",
            &[("a.txt", false), ("src", true)],
        ),
    ];
    for (command, expected) in table {
        let (paths, _, _) = proven_full(command);
        let expected: Vec<(String, bool)> =
            expected.iter().map(|(p, r)| (p.to_string(), *r)).collect();
        assert_eq!(paths, expected, "{command}");
    }
}

#[test]
fn a_read_of_an_obvious_secret_path_is_not_proven() {
    assert_none_proven(&[
        // ssh and gnupg, as an operand, an absolute path, a directory, a sub path
        "cat /home/tester/.ssh/id_ed25519",
        "cat .ssh/config",
        "ls /home/tester/.ssh",
        "ls /home/tester/.ssh/",
        "cat ./sub/.ssh/known_hosts",
        "ls .gnupg",
        "cat /home/tester/.gnupg/pubring.kbx",
        // env files
        "cat .env",
        "cat ./.env",
        "cat sub/.env",
        "cat .env.local",
        "cat .env.production",
        "head -n 3 .env",
        "grep KEY .env",
        // key and certificate files
        "cat server.pem",
        "cat certs/server.key",
        "cat id_rsa",
        "cat sub/id_ed25519",
        "cat id_ecdsa.pub",
        // cloud and package credentials
        "cat /home/tester/.aws/credentials",
        "jq . /home/tester/.aws/credentials",
        "cat .aws/credentials",
        "cat .netrc",
        "cat /home/tester/.npmrc",
        "cat .pypirc",
        "cat .git-credentials",
        // every way a path is named
        "cat < .env",
        "cat -- .env",
        "grep -f .env foo",
        "grep -e x .env",
        "git log -- .env",
        "git diff HEAD -- .ssh/config",
        "find . -newer .env",
        "diff .env other",
        "realpath --relative-to=.env a",
        "cd /home/tester/.ssh && ls",
        "cd ./.aws && cat credentials",
        // a recursive walk that starts at a secret directory
        "rg KEY /home/tester/.ssh",
        "find /home/tester/.ssh -type f",
        "grep -r x .aws/credentials",
        // quoted: the name is the same
        "cat '.env'",
        "cat \".ssh/id_rsa\"",
        // a glob that can expand to one
        "cat .en*",
        "cat .e?v",
        "cat .env*",
        "cat ./*.pem",
        "cat src/*.key",
        "cat id_*",
        "cat .ss[h]/id_rsa",
        "ls .ssh*",
        "cat ./*_rsa",
        "cat ./*.pem*",
        "cat ./id*",
        "cat ./.e*",
        "cat .net*",
    ]);
    // the neighbours are plain
    for command in [
        "cat env.rs",
        "cat .environment",
        "cat .envrc.md",
        "cat envfile",
        "cat monkey.rs",
        "cat keys.rs",
        "cat key.rs",
        "cat pem.txt",
        "cat id.rs",
        "cat video_id.txt",
        "cat identity",
        "cat credentials.md",
        "cat .aws/config",
        "cat aws/credentials",
        "cat ssh/config",
        "cat src/*.rs",
        "cat ./*",
        "cat src/*",
        "cat s*",
        "cat x*",
        "ls /tmp/x*",
        "ls src/*.md",
        "ls src/*.p*y",
        "cat .git/HEAD",
        "git log -- src",
        "ls /home/tester/projects",
        "cd /home/tester/projects && ls",
    ] {
        proven(command);
    }
}

#[test]
fn the_secret_pattern_const_and_its_matcher_agree() {
    assert!(SECRET_PATTERNS.len() >= 19, "the list is kept whole");
    for pattern in SECRET_PATTERNS {
        // every pattern is itself a secret, as a name and as a sub path
        let name = pattern.replace('*', "x");
        assert!(is_secret_path(&name), "{pattern}: {name}");
        assert!(is_secret_path(&format!("a/b/{name}")), "{pattern}");
        assert!(is_secret_path(&format!("/abs/{name}/more")), "{pattern}");
        assert!(is_secret_path(&format!("./{name}")), "{pattern}");
        assert!(is_secret_path(&format!("{name}/")), "{pattern}");
        // and no pattern is empty or rooted
        assert!(
            !pattern.is_empty() && !pattern.starts_with('/'),
            "{pattern}"
        );
    }
    for named in [
        ".ssh",
        ".gnupg",
        ".aws/credentials",
        ".env",
        ".env.*",
        "*.pem",
        "*.key",
        "id_*",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        ".config/gh/hosts.yml",
        ".docker/config.json",
        ".kube/config",
        ".claude/.credentials.json",
        ".bash_history",
        ".zsh_history",
        ".histfile",
    ] {
        assert!(SECRET_PATTERNS.contains(&named), "{named} is on the list");
    }
    for plain in [
        "",
        ".",
        "src/lib.rs",
        "README.md",
        ".environment",
        "pem",
        "key",
        "monkey",
        "id",
        "aws/credentials",
        ".aws/config",
        "credentials",
        "/etc/hosts",
        "*",
        "src/*",
        "?",
        "[a-z]*.rs",
    ] {
        assert!(!is_secret_path(plain), "{plain:?}");
    }
    // the dot segments and a doubled slash do not hide a name
    assert!(is_secret_path("./././.env"));
    assert!(is_secret_path("a//.env"));
    assert!(is_secret_path("a/./.ssh/./id"));
}

#[test]
fn the_secret_matcher_never_panics_and_never_refuses_a_plain_name() {
    // a very long glob is read in linear time
    let stars = "*a".repeat(2000);
    assert!(!is_secret_path(&stars));
    assert!(!is_secret_path(&"a/".repeat(2000)));
    assert!(!is_secret_path("["));
    assert!(!is_secret_path("a["));
    assert!(!is_secret_path(".env["));
}

#[test]
fn the_tables_are_written_for_the_programs_that_really_run() {
    // The shell snapshot the commands run under puts bfs in place of find and ugrep in place of grep, and rg
    // may be its own build; the system tools are the macOS BSD ones. An option the tables list means the same on every
    // one of them. These are the options of those programs that run or write, or that read another file, and
    // none of them is listed.
    assert_none_proven(&[
        // ugrep: an interactive query UI, a pager, a filter command, a decompressor, config files
        "grep -Q foo",
        "grep --query foo",
        "grep --pager foo a.txt",
        "grep --filter=x foo a.txt",
        "grep -z foo a.txt",
        "grep -Z foo a.txt",
        "grep --decompress foo a.txt",
        "grep --config=x foo a.txt",
        "grep --save-config foo",
        "grep --tag foo a.txt",
        "grep --hexdump foo a.txt",
        "grep --stats foo a.txt",
        "grep -X foo a.txt",
        "grep --index foo a.txt",
        "grep --ignore-files=x foo a.txt",
        "grep --exclude-from=x foo a.txt",
        "grep -R foo src",
        "grep -S foo src",
        // bfs: the actions and the options that change how it walks
        "find . -exec echo {} ;",
        "find . -fprint out.txt",
        "find . -fprint0 out.txt",
        "find . -fls out.txt",
        "find . -delete",
        "find . -rm",
        "find . -nohidden",
        "find . -unique",
        "find . -sort",
        "find -L .",
        "find -H .",
        "find -S dfs .",
        "find . -files0-from x",
        "find . -newer-x a",
        // ripgrep: a preprocessor, a config file, decompression, a hyperlink command
        "rg --pre cat foo",
        "rg --pre=cat foo",
        "rg -z foo",
        "rg --search-zip foo",
        "rg --hostname-bin x foo",
        "rg --config-path x foo",
        "rg --engine=x foo",
        // BSD file and stat: reading from a list, following links
        "file -f list",
        "file -p a.png",
        "file --preserve-date a.png",
        "file -z a.gz",
        "stat -f %N a",
    ]);
    // the programs themselves are the bare names; bfs and ugrep are no table program
    assert_none_proven(&[
        "bfs . -name x",
        "ugrep foo a.txt",
        "ug foo a.txt",
        "gfind . -name x",
    ]);
    for program in ["bfs", "ugrep", "ug", "gfind", "ggrep"] {
        assert!(!READONLY_PROGRAMS.contains(&program), "{program}");
    }
    // the options the tables do list
    for command in [
        "grep -rn foo src",
        "grep -c foo a.txt",
        "rg -n foo src",
        "find . -name x -type f",
        "file -b a.png",
        "stat -c %s a",
    ] {
        proven(command);
    }
}

// --- review round 2 --------------------------------------------------------------------------------------------

#[test]
fn grep_options_the_snapshot_grep_reads_differently_are_not_proven() {
    // The grep that runs is ugrep: there `-NUM` is a recursion depth that turns on `-r`, where GNU reads it
    // as context lines, and `-d` takes `dereference-recurse`. GNU takes abbreviations of the `-d` values.
    assert_none_proven(&[
        "grep -5 KEY src",
        "grep -1 KEY src",
        "grep -n -2 KEY src",
        "egrep -5 KEY src",
        "fgrep -5 KEY src",
        "grep -d dereference-recurse KEY src",
        "grep -ddereference-recurse KEY src",
        "grep --directories=dereference-recurse KEY src",
        "grep -d rec KEY src",
        "grep -drec KEY src",
        "grep -d r KEY src",
        "grep -d re KEY src",
        "grep -d s KEY src",
        "grep --directories=recu KEY src",
        "grep --directories=rec KEY src",
        "grep --directories rec KEY src",
        "grep --directories=sk KEY src",
        "grep -d Recurse KEY src",
        "grep -d recurse2 KEY src",
        "grep -d '' KEY src",
        "grep -d KEY src",
        "grep --directories= KEY src",
        // the same rule for the devices action
        "grep -D rec KEY src",
        "grep -D foo KEY src",
        "grep --devices=sk KEY src",
    ]);
    // exactly `read`, `skip` and `recurse`; `recurse` walks
    for command in [
        "grep -d read KEY src",
        "grep -d skip KEY src",
        "grep -dskip KEY src",
        "grep --directories=read KEY src",
        "grep --directories skip KEY src",
        "grep -D read KEY src",
        "grep --devices=skip KEY src",
        "grep -C 5 KEY src",
        "grep -A5 KEY src",
        "grep --context=5 KEY src",
    ] {
        assert!(!proven_full(command).0[0].1, "{command}");
    }
    for command in [
        "grep -d recurse KEY src",
        "grep -drecurse KEY src",
        "grep --directories=recurse KEY src",
        "grep --directories recurse KEY src",
    ] {
        assert!(proven_full(command).0[0].1, "{command}");
    }
    // `-NUM` stays an option of head, tail and git log
    proven("head -5 a.txt");
    proven("tail -20 a.txt");
    proven("git log -5");
}

#[test]
fn two_directories_given_to_diff_are_read_in_full() {
    // `diff ./a ./b` reads every file the two folders share, a `.env` included
    assert_eq!(
        proven_full("diff ./a ./b").0,
        [("./a".to_string(), true), ("./b".to_string(), true)]
    );
    // the secret check sees what a recursive read reaches: a directory that holds a secret by name
    assert_none_proven(&[
        "diff .aws other",
        "diff other .aws",
        "diff ./.docker ./b",
        "diff /srv/app/.kube /srv/app/x",
        "diff -u .config/gh x",
        "diff .claude x",
    ]);
    proven("diff -u a.txt b.txt");
    proven("diff .config/other x");
}

#[test]
fn a_recursive_read_that_starts_above_a_secret_is_not_proven() {
    // a pattern of several components is reached from the directory that holds it
    assert_none_proven(&[
        "rg KEY .aws",
        "rg KEY ./.aws",
        "rg KEY /srv/app/.aws",
        "rg KEY .config",
        "rg KEY .config/gh",
        "rg KEY .config/gh/",
        "rg KEY .docker",
        "rg KEY .kube",
        "rg KEY .claude",
        "grep -r KEY .claude",
        "find .kube -type f",
        "find ./.docker -name x",
        "du -sh .claude",
        "tree .config",
        "ls -R .aws",
        "git log -- .kube",
        "cd ./.aws && rg KEY",
        "cd ./.config && cd ./gh && rg KEY",
        "cd /srv/app/.docker && find . -type f",
        "cd ./.kube && tree",
    ]);
    // the same directory read without walking it holds no secret by name
    for command in [
        "ls .aws",
        "ls -la .kube",
        "ls .config",
        "cd ./.aws && ls",
        "cd ./.config && ls gh",
        "cat .config/other",
        "ls .claude",
        "rg KEY .claude/agent-memory",
        "rg KEY .config/other",
        "rg KEY .aws-extra",
        "find .claude/agent-memory -type f",
    ] {
        proven(command);
    }
}

#[test]
fn the_secret_names_are_matched_without_regard_to_ascii_case() {
    // APFS and the default macOS volume ignore case: `.ENV` is `.env`
    assert_none_proven(&[
        "cat .ENV",
        "cat .Env",
        "cat .ENV.LOCAL",
        "cat .AWS/CREDENTIALS",
        "cat .Aws/Credentials",
        "cat ID_RSA",
        "cat Id_Ed25519",
        "cat SERVER.PEM",
        "cat Server.Key",
        "cat .NETRC",
        "cat .NPMRC",
        "cat .PYPIRC",
        "cat .GIT-CREDENTIALS",
        "cat .SSH/config",
        "ls /srv/app/.Ssh",
        "cat .GNUPG/x",
        "cat .En*",
        "cat .EN?",
        "cat ./ID*",
        "cat ./.SS[H]/x",
        "cat ./*_RSA",
        "cat ./*.PEM",
        "cat '.ENV'",
        "cd ./.SSH && cat config",
        "cd ./.AWS && cat CREDENTIALS",
        "cat .Config/Gh/Hosts.Yml",
        "cat .DOCKER/CONFIG.JSON",
        "cat .KUBE/CONFIG",
        "cat .CLAUDE/.CREDENTIALS.JSON",
        "cat .BASH_HISTORY",
        "cat .ZSH_HISTORY",
        "cat .HISTFILE",
        "rg KEY .AWS",
        "rg KEY .Config/GH",
    ]);
    for command in ["cat .ENVIRONMENT", "cat ENV.RS", "cat Video_ID.TXT"] {
        proven(command);
    }
    assert!(is_secret_path(".ENV"));
    assert!(is_secret_path("A/B/.Env.Local"));
    assert!(is_secret_path(".Aws/Credentials"));
    assert!(!is_secret_path(".ENVIRONMENT"));
}

#[test]
fn more_credential_and_history_files_are_not_proven() {
    assert_none_proven(&[
        "cat .config/gh/hosts.yml",
        "cat /srv/app/.config/gh/hosts.yml",
        "cat .docker/config.json",
        "cat .kube/config",
        "cat .claude/.credentials.json",
        "cat .bash_history",
        "cat .zsh_history",
        "cat .histfile",
        "tail -n 5 /srv/app/.zsh_history",
        "grep ssh .bash_history",
        "head -n 3 .histfile",
        "jq . .claude/.credentials.json",
        "git log -- .docker/config.json",
        "cd ./.kube && cat config",
        "cd /srv/app/.config/gh && cat hosts.yml",
    ]);
    for command in [
        "cat .config/gh/other.yml",
        "cat .docker/other.json",
        "cat .kube/cache",
        "cat .claude/settings.json",
        "cat .claude/agent-memory/x.md",
        "cat bash_history",
        "cat history.md",
        "cat .zsh_history.md",
    ] {
        proven(command);
    }
}

#[test]
fn a_glob_that_could_match_a_dotfile_under_dotglob_is_not_proven() {
    // The glob check holds a glob against secret names as if `*` could match a leading `.` (zsh with
    // GLOB_DOTS, bash with dotglob): `*env` is `.env` there.
    assert_none_proven(&[
        "cat ./*env",
        "cat ./?env",
        "cat ./*rc",
        "cat ./*_history",
        "cat ./*history",
        "cat ./*.local",
        "cat ./*ssh",
        // a glob under a directory that holds a secret by name
        "cat .kube/*",
        "cat .kube/c*",
        "cat .aws/*",
        "cat .config/gh/*",
        "cat .claude/*",
        "cat ./.docker/*",
        "cat /srv/app/.kube/?onfig",
    ]);
    // a glob with nothing literal in a path with nothing secret stays plain
    for command in [
        "cat ./*",
        "cat src/*",
        "cat src/*/x.rs",
        "cat ./*/*.md",
        "cat ./*/*",
        "cat s*",
        "cat ./*.rs",
        "cat ./*.json",
        "cat .config/other/*",
        "cat .claude/agent-memory/*",
    ] {
        proven(command);
    }
}

#[test]
fn a_git_format_that_runs_gpg_or_ssh_keygen_is_not_proven() {
    // `%G?`, `%GS`, `%GK` and the rest of the `%G` placeholders verify a signature: gpg or ssh-keygen runs
    assert_none_proven(&[
        "git log --pretty='%GS %G?'",
        "git log --pretty=format:%G?",
        "git log --pretty=format:'%h %G?'",
        "git log --format=%GK",
        "git log --format='%GS'",
        "git log --pretty=tformat:%GG",
        "git show --pretty=format:%GF HEAD",
        "git show --format=%GP HEAD",
        "git log -3 --format='%h %GT'",
        "git log --pretty=\"%G?\"",
        "git branch --format=%GS",
        "git log --pretty=%%G",
    ]);
    // the neighbours: `%g` (reflog) and every other placeholder
    for command in [
        "git log --pretty=format:%h",
        "git log --pretty='%h %s %an'",
        "git log --format=%gD",
        "git log --format=%gs",
        "git log --pretty=format:%H%n%b",
        "git show --format=%cd HEAD",
    ] {
        proven(command);
    }
}

// --- the bypass corpus and the guard's payloads -------------------------------------------------------------

const SEP: &str = "\n#---\n";

fn cases(text: &str) -> Vec<&str> {
    text.split(SEP)
        .map(|c| c.trim_end_matches('\n'))
        .filter(|c| !c.trim().is_empty())
        .collect()
}

#[test]
fn every_verified_bypass_payload_is_not_proven() {
    let all = cases(include_str!("fixtures/sh_fw_verify_payloads.txt"));
    assert!(all.len() > 300, "the corpus is read: {} cases", all.len());
    let wrong: Vec<String> = all
        .into_iter()
        .filter(|c| matches!(classify(c), ReadOnly::Proven { .. }))
        .map(|c| format!("{c:?}"))
        .collect();
    assert!(wrong.is_empty(), "proven:\n{}", wrong.join("\n"));
}

#[test]
fn every_fw_h_blocked_payload_is_not_proven() {
    // a JSON array of strings; read without a JSON crate (the shell crate has none): each element is one
    // line ` "…",`, with `\n`, `\\` and `\"` escapes
    let text = include_str!("fixtures/fw_h_base_blocked.json");
    let mut seen = 0;
    let mut wrong = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(body) = line.strip_prefix('"') else {
            continue;
        };
        let body = body.trim_end_matches(',').trim_end_matches('"');
        let mut command = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                command.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => command.push('\n'),
                Some('t') => command.push('\t'),
                Some('r') => command.push('\r'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
                    command.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                Some(other) => command.push(other),
                None => {}
            }
        }
        seen += 1;
        if matches!(classify(&command), ReadOnly::Proven { .. }) {
            wrong.push(format!("{command:?}"));
        }
    }
    assert!(seen > 300, "the corpus is read: {seen} cases");
    assert!(wrong.is_empty(), "proven:\n{}", wrong.join("\n"));
}

// --- fail closed: never a panic, and nothing proven that the parser did not read completely -------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    /// Any text: no panic, and a proven command never holds a character that changes what runs.
    #[test]
    fn arbitrary_text_never_panics_and_a_proven_command_has_no_metacharacter(text in "\\PC{0,80}") {
        if let ReadOnly::Proven { .. } = classify(&text) {
            let unquoted = strip_quoted(&text);
            for bad in [';', '`', '$', '\\', '(', ')', '{', '}', '~', '^', '#', '!', '\n', '\r', '\0'] {
                prop_assert!(!unquoted.contains(bad), "{text:?} proven with {bad:?}");
            }
        }
    }

    /// Shell-flavoured soup: the same property over text built from the tokens that matter.
    #[test]
    fn shell_soup_never_panics_and_a_proven_command_is_a_table_program(
        words in proptest::collection::vec(
            prop_oneof![
                Just("ls"), Just("cat"), Just("git"), Just("log"), Just("find"), Just("cd"), Just("tee"),
                Just("-exec"), Just("-delete"), Just("-o"), Just("-n"), Just("5"), Just("--output=x"),
                Just("&&"), Just("||"), Just(";"), Just("|"), Just("&"), Just(">"), Just(">>"), Just("<"),
                Just("2>&1"), Just(">/dev/null"), Just("$(x)"), Just("`x`"), Just("$X"), Just("'a b'"),
                Just("\"q\""), Just("../x"), Just("src"), Just("/tmp"), Just("./a"), Just("*.rs"), Just(".*"),
                Just("~"), Just("{a,b}"), Just("<(x)"), Just("A=b"), Just("\n"), Just("sed"), Just("-i"),
                Just("head"), Just("grep"), Just("rg"), Just("--pre"), Just("sort"), Just("-T"),
            ],
            0..10,
        ),
    ) {
        let command = words.join(" ");
        if let ReadOnly::Proven { paths, cd, .. } = classify(&command) {
            let paths: Vec<String> = paths.into_iter().map(|p| p.path).collect();
            let analysis = analyze(&command, HOME);
            prop_assert!(!analysis.incomplete, "{command:?}");
            prop_assert!(analysis.alternates.is_empty(), "{command:?}");
            for entry in &analysis.entries {
                prop_assert!(entry.opaque.is_empty(), "{command:?}");
                prop_assert!(
                    READONLY_PROGRAMS.contains(&entry.prog.as_str()) || entry.prog == "cd",
                    "{command:?}: {entry:?}"
                );
            }
            for path in paths.iter().chain(&cd) {
                prop_assert!(!path.split('/').any(|c| c == ".."), "{command:?}: {path}");
                prop_assert!(!is_secret_path(path), "{command:?}: {path}");
            }
            for target in &cd {
                prop_assert_eq!(literal_cd_target(target), Some(target.as_str()), "{}", command);
            }
        }
    }

    /// `literal_cd_target` and `head_tail_count` never panic and never accept a word with a metacharacter.
    #[test]
    fn the_helpers_never_panic(word in "\\PC{0,40}") {
        if let Some(target) = literal_cd_target(&word) {
            prop_assert_eq!(target, word.as_str());
            prop_assert!(word.starts_with('/') || word.starts_with("./"));
            prop_assert!(!word.split('/').any(|c| c == ".."));
            prop_assert!(word.chars().all(|c| c.is_ascii_alphanumeric() || "/._+,:@%=-".contains(c)));
        }
        let _ = head_tail_count(std::slice::from_ref(&word));
        let _ = head_tail_count(&["-n".to_string(), word]);
    }
}

/// The text with every quoted span removed, for the metacharacter property (single quotes hide anything,
/// double quotes hide all but `$`, a backquote, a backslash and `!`, which are refused inside them too).
fn strip_quoted(text: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if matches!(c, '$' | '`' | '\\' | '!') => out.push(c),
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => out.push(c),
        }
    }
    out
}
