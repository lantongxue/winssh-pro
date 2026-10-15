//! 采集脚本：一次 `exec` 就把右侧监控面板要的所有读数取回来。
//!
//! 数据源是 `/proc`，不是 `top`。`top` 的输出格式随 procps 版本、locale 与
//! busybox 实现而变，拿它当接口迟早会碎；`/proc/stat`、`/proc/meminfo`、
//! `/proc/[pid]/stat` 是内核 ABI，十几年没动过。
//!
//! **CPU 占用必须是两次采样之差**：`/proc` 里的都是开机以来的累计值，单次快照
//! 只能算出"开机至今的平均值"，那和"现在忙不忙"是两回事。所以脚本自己在远端
//! 采两次、间隔 0.3 秒，把窗口内的差值算出来 —— 这也正是 `top` 干的事
//! （`top` 的第一帧同样只有累计值，所以它默认要跑第二帧）。进程列表在远端就
//! 按 CPU 排好序；Metrics 用的默认档位截断到前 N 条，回传的字节数与主机上有
//! 多少进程无关，进程管理那一页则显式要全部（[`ProbeOptions::all_processes`]，
//! 只排序不截断）。
//!
//! 面板收起来的时候用 [`ProbeOptions::light`]：省掉那 0.3 秒的采样窗口与进程
//! 列表，只取状态栏要的那几个数，一次往返几百字节。
//!
//! 段落用 `##name` 分隔，解析在 [`super::sample`]。两边必须同时改。

/// 一次采集要取哪些东西。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeOptions {
    /// 进程列表要前多少条，0 表示不采（省掉远端的采样窗口），
    /// [`ALL_PROCESSES`] 表示不截断、回传全部进程（Procs 那一页用）。
    pub top_processes: usize,
}

/// 进程列表不截断的哨兵值：进程管理那一页要看到主机上所有进程。
pub const ALL_PROCESSES: usize = usize::MAX;

/// 面板收起来时：只要状态栏那几个数。
impl ProbeOptions {
    pub const fn light() -> Self {
        Self { top_processes: 0 }
    }

    /// 面板展开时：带进程列表。
    pub const fn full() -> Self {
        Self {
            top_processes: TOP_PROCESSES,
        }
    }

    /// 进程管理那一页展开时：回传全部进程，按 CPU 排序但不截断。
    pub const fn all_processes() -> Self {
        Self {
            top_processes: ALL_PROCESSES,
        }
    }
}

/// 进程列表默认取前多少条。Metrics 页显示 CPU 最高的 10 条，40 条留着给
/// "View All" 的预览，也不至于让一次采集变成几十 KB。进程管理那一页要看到
/// 全部进程，走 [`ProbeOptions::all_processes`]（不截断）。
pub const TOP_PROCESSES: usize = 40;

/// 远端采样窗口的长度。太短则 CPU 百分比抖得没法读，太长则每次采集都要等它。
const WINDOW: &str = "0.3";

/// 段落标记。解析端按它切块，改一处必须改两处。
pub const SECTION_PREFIX: &str = "##";
/// 段落内部的第二段分隔（同一段里的两次采样）。
pub const SPLIT: &str = "@@";
pub const SECTION_END: &str = "end";

/// 拼出这次采集的 POSIX sh 脚本。
pub fn script(options: &ProbeOptions) -> String {
    let mut script = String::with_capacity(2048);
    script.push_str(PRELUDE);

    if options.top_processes > 0 {
        script.push_str(WINDOW_OPEN);
        script.push_str("sleep ");
        script.push_str(WINDOW);
        script.push_str(WINDOW_CLOSE);
    }

    script.push_str(BODY);

    if options.top_processes > 0 {
        // `$top` 先收全量、按 CPU 排好序。截断只发生在回传的那份列表上：
        // Metrics 的"高负载前 10"、Procs 预览那几十条。真正的总数走 `$total`，
        // 单独回传一段 `##proccount`，标签上的 "Procs (N)" 要用它而不是截断后的
        // 列表长度 —— 否则切走 Processes 那一页、下一轮采集接了 `head`，标签就
        // 卡在 40 上了。`ALL_PROCESSES`（进程管理那一页）本来就不截断。
        script.push_str(PROC_HEAD);
        script.push_str(PROC_AWK);
        script.push_str(" | sort -k2,2nr)\n");
        script.push_str(PROC_COUNT_OPEN);
        if options.top_processes != ALL_PROCESSES {
            script.push_str("top=$(printf '%s\\n' \"$top\" | head -n ");
            script.push_str(&options.top_processes.to_string());
            script.push_str(")\n");
        }
        script.push_str(PROC_TAIL);
        script.push_str(PROC_COUNT_EMIT);
    }

    script.push_str("echo '");
    script.push_str(SECTION_PREFIX);
    script.push_str(SECTION_END);
    script.push_str("'\n");
    script
}

/// 环境与常量。`LC_ALL=C` 是为了小数点是 `.`、月份名是英文 —— 解析端按固定
/// 格式读，不能跟着对端的 locale 变。`PATH` 补齐是因为非交互 sh 拿到的 PATH
/// 可能不含 `/usr/sbin`，`df`、`ps` 在有些发行版上就在那儿。
const PRELUDE: &str = "\
LC_ALL=C
export LC_ALL
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH
export PATH
hz=$(getconf CLK_TCK 2>/dev/null)
case \"$hz\" in ''|*[!0-9]*) hz=100 ;; esac
pg=$(getconf PAGESIZE 2>/dev/null)
case \"$pg\" in ''|*[!0-9]*) pg=4096 ;; esac
pgkb=$((pg / 1024))
u1=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
c1=$(grep '^cpu' /proc/stat 2>/dev/null)
";

/// 采样窗口的前半：先把所有进程的累计 CPU 时间抓下来。
const WINDOW_OPEN: &str = "\
p1=$(cat /proc/[0-9]*/stat 2>/dev/null)
";

/// 采样窗口的后半。`sleep 0.3` 不是 POSIX 必须支持的小数秒，不认就退回整秒
/// —— 窗口实际有多长由 `/proc/uptime` 的两次读数说了算，退化了也算得对。
const WINDOW_CLOSE: &str = " 2>/dev/null || sleep 1
p2=$(cat /proc/[0-9]*/stat 2>/dev/null)
u2=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)
c2=$(grep '^cpu' /proc/stat 2>/dev/null)
";

/// 不依赖采样窗口的那些读数。`##cpu` 段里 `@@` 之后是窗口末尾的那次采样，
/// 轻量采集时那里是空的，由解析端退回"与上一次采集比"。
const BODY: &str = "\
echo '##hz'
echo \"$hz $pgkb\"
echo '##window'
echo \"$u1 ${u2:-$u1}\"
echo '##uptime'
cat /proc/uptime 2>/dev/null
echo '##load'
cat /proc/loadavg 2>/dev/null
echo '##cpu'
echo \"$c1\"
echo '@@'
echo \"$c2\"
echo '##mem'
grep -E '^(MemTotal|MemFree|MemAvailable|Buffers|Cached|SwapTotal|SwapFree):' /proc/meminfo 2>/dev/null
echo '##net'
cat /proc/net/dev 2>/dev/null
echo '##disk'
df -kP / 2>/dev/null
";

const PROC_HEAD: &str = "\
echo '##proc'
top=$(printf '%s\\n@@\\n%s\\n' \"$p1\" \"$p2\" | ";

/// 窗口内的 CPU 差值。`/proc/[pid]/stat` 的第 2 个字段是 `(comm)`，进程名里
/// 可以有空格甚至括号，所以不能按空白切 —— 先找到**最后**一个 `)`，它后面才是
/// 定长字段：state 是第 3 个、utime 第 14、stime 第 15、rss 第 24。
const PROC_AWK: &str = "awk -v hz=\"$hz\" -v pgkb=\"$pgkb\" -v u1=\"$u1\" -v u2=\"${u2:-$u1}\" '\
BEGIN { dt = u2 - u1; if (dt <= 0.02) dt = 0.3 }
$0 == \"@@\" { pass = 2; next }
NF < 20 { next }
{
  pid = $1
  o = index($0, \"(\")
  p = 0
  while ((k = index(substr($0, p + 1), \")\")) > 0) p += k
  if (o < 1 || p <= o) next
  name = substr($0, o + 1, p - o - 1)
  n = split(substr($0, p + 2), f, \" \")
  if (n < 22) next
  t = f[12] + f[13]
  if (pass == 2) { t2[pid] = t; st[pid] = f[1]; rs[pid] = f[22]; nm[pid] = name }
  else t1[pid] = t
}
END {
  for (pid in t2) {
    base = (pid in t1) ? t1[pid] : t2[pid]
    d = t2[pid] - base
    if (d < 0) d = 0
    printf \"%s\\t%.1f\\t%s\\t%d\\t%s\\n\", pid, d * 100 / (hz * dt), st[pid], rs[pid] * pgkb, nm[pid]
  }
}'";

/// 用户名与完整命令行只对上面选出来的那几十个进程查，`ps` 的输出在远端就按
/// pid 过滤掉，回传的还是那几十行。内存百分比不问 `ps`：`/proc` 段已经给了
/// rss，除以 `MemTotal` 算出来的是同一个数。`ps` 不在（或不认 `-e`）时这一段
/// 是空的，解析端退回 `/proc` 里的进程名。
const PROC_TAIL: &str = "\
printf '%s\\n' \"$top\"
echo '##ps'
pids=$(printf '%s\\n' \"$top\" | cut -f1 | tr '\\n' ' ')
ps -eo pid=,user=,args= 2>/dev/null | awk -v list=\"$pids\" 'BEGIN { n = split(list, a, \" \"); for (i = 1; i <= n; i++) keep[a[i]] = 1 } keep[$1]'
";

/// 在截断之前，先数一下全量列表里到底有多少进程。`grep -c .` 数的是非空行，
/// 恰好等于 `$top` 的条数（`printf` 补的那个换行不产生额外非空行）。
const PROC_COUNT_OPEN: &str = "total=$(printf '%s\\n' \"$top\" | grep -c .)\n";

/// 真正的进程总数单独一段，放在进程段之后 —— 每一段读到下一个 `##` 为止，
/// 段落之间的先后不影响解析。
const PROC_COUNT_EMIT: &str = "echo '##proccount'\necho \"$total\"\n";

#[cfg(test)]
#[path = "tests/monitor/probe.rs"]
mod tests;
