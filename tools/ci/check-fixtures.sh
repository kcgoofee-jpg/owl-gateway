#!/usr/bin/env bash
# 公开仓库红线：测试快照、测试与开发脚本里只允许合成数据。
# 拦截：17–20 位 Discord 雪花 ID、公网 IPv4、看起来像真实上游 Token 的串。
# 放行：合成 ID（9000000000000000xx 段；首位之后连着 6 个以上 0，如 100000000000000001；
#       1234567890… 顺序串；u64 边界值）、
#       私网 / 文档网段 / 众所周知的占位地址（1.2.3.4、8.8.8.8、9.9.9.9、1.1.1.1）、
#       更长点分数字串里的片段、浏览器版本号（Chrome/129.0.0.0）。
#       errors/signature.jsonl 是「3.5」「9.」「05」等记号随机拼成的模糊测试串，不查 IP。
# 不允许 .gz 夹具（压缩字节里看不出明文，扫不了）：见到直接失败。git 已跟踪和未跟踪（未忽略）的文件
# 都查，本地新生成、还没 git add 的快照也算。
#
#   tools/ci/check-fixtures.sh              扫描仓库
#   tools/ci/check-fixtures.sh --self-test  先证明规则真的会拦（CI 里在正式扫描前跑）
set -euo pipefail
cd "$(dirname "$0")/../.."

scan() {
  # 参数是文件列表；有违规时退出码 1
  perl -e '
    use strict; use warnings;
    my %ok = map { $_ => 1 } qw(1.2.3.4 8.8.8.8 9.9.9.9 1.1.1.1 127.0.0.1 0.0.0.0);
    my ($fail, $files) = (0, 0);
    for my $f (@ARGV) {
      open(my $fh, "<", $f) or die "$f: 打不开\n";
      $files++;
      my $fuzz = $f =~ m{errors/signature\.jsonl$};
      while (my $line = <$fh>) {
        while ($line =~ /"([1-8][0-9]{16,19})"/g) {
          my $id = $1;
          next if $id =~ /^[1-9]0{6,}[0-9]*$/ || "12345678901234567890" =~ /^\Q$id\E/ || $id =~ /^1844674407370955161[56]$/;
          print STDERR "$f:$.: 疑似真实 Discord ID $id（合成数据请用 9000000000000000xx）\n"; $fail = 1;
        }
        while (!$fuzz && $line =~ /(?<![0-9.\/])((?:[0-9]{1,3}\.){3}[0-9]{1,3})(?!\.?[0-9])/g) {
          my $ip = $1; my @o = split /\./, $ip;
          next if grep { $_ > 255 } @o;
          next if $ok{$ip};
          next if $o[0] == 10 || ($o[0] == 192 && $o[1] == 168) || ($o[0] == 172 && $o[1] >= 16 && $o[1] <= 31);
          next if "$o[0].$o[1].$o[2]" =~ /^(192\.0\.2|198\.51\.100|203\.0\.113)$/;
          next if $o[0] == 127 || $o[0] == 0;
          print STDERR "$f:$.: 疑似公网 IP $ip（测试请用 RFC 5737 文档网段 203.0.113.0/24 等）\n"; $fail = 1;
        }
        if ($line =~ /pst-[A-Za-z0-9_-]{40,}/) { print STDERR "$f:$.: 疑似真实上游 Token\n"; $fail = 1 }
      }
      close($fh) or do { print STDERR "$f: 读取出错\n"; $fail = 1 };
    }
    print STDERR "扫描 $files 个文件\n";
    exit $fail;
  ' "$@"
}

if [ "${1:-}" = "--self-test" ]; then
  dir=$(mktemp -d)
  trap 'rm -rf "$dir"' EXIT
  printf '{"ip":"8.8.4.4"}\n' > "$dir/ip.jsonl"
  printf '{"id":"112233445566778899"}\n' > "$dir/id.jsonl"
  printf '{"ip":"203.0.113.7","id":"900000000000000001"}\n' > "$dir/clean.jsonl"
  for f in ip.jsonl id.jsonl; do
    if scan "$dir/$f" 2>/dev/null; then echo "自检失败：$f 应该被拦下" >&2; exit 1; fi
  done
  scan "$dir/clean.jsonl" 2>/dev/null || { echo "自检失败：clean.jsonl 不该被拦" >&2; exit 1; }
  echo "自检通过：明文公网 IP / Discord ID 会被拦，合成数据放行"
  exit 0
fi

paths=(crates/*/tests crates/xtask web/e2e web/src)
files=()
while IFS= read -r -d '' f; do
  case "$f" in
    *.gz) echo "$f: 不允许提交 .gz 夹具（扫不了压缩内容），请存明文" >&2; exit 1 ;;
    *.png | *.jpg | *.jpeg | *.webp | *.gif | *.ico | *.woff | *.woff2 | *.zip) continue ;;
  esac
  [ -f "$f" ] && files+=("$f")
done < <(git ls-files -z --cached --others --exclude-standard -- "${paths[@]}")
[ ${#files[@]} -eq 0 ] && exit 0
scan "${files[@]}"
