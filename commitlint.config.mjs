/** @type {import('@commitlint/types').UserConfig} */

// 提交标题只允许中文正文，不允许中文句末标点。
// Conventional Commits 的解析依赖 ASCII 的 `type(scope): `，所以这部分必须保持英文。
//
// 改动下面的 type 或 scope 列表时，同步更新这三处：
//   .gitmessage、AGENTS.md 的「提交规则」、CONTRIBUTING.md 的「提交信息」。
const noChineseTerminalPunctuation = (parsed) => {
  const subject = parsed.subject ?? '';
  const bad = /[。！？；，]$/.test(subject);
  return [!bad, '标题结尾不要加中文标点（。！？；，）'];
};

export default {
  extends: ['@commitlint/config-conventional'],
  plugins: [
    {
      rules: {
        'subject-no-chinese-punctuation': noChineseTerminalPunctuation,
      },
    },
  ],
  rules: {
    'type-enum': [
      2,
      'always',
      ['feat', 'fix', 'perf', 'refactor', 'docs', 'test', 'build', 'ci', 'chore', 'revert'],
    ],
    'scope-enum': [
      2,
      'always',
      [
        'doc',
        'cmd',
        'engine',
        'graph',
        'dsp',
        'io',
        'host',
        'session',
        'proto',
        'script',
        'ui',
        'cli',
        'sdk',
        'ci',
        'deps',
        'repo',
        'release',
      ],
    ],
    'scope-empty': [2, 'never'],
    'subject-empty': [2, 'never'],
    // 中文没有大小写，这条规则只会误伤英文缩写。
    'subject-case': [0],
    'subject-full-stop': [2, 'never', '.'],
    'subject-no-chinese-punctuation': [2, 'always'],
    // 中文一个字占两列，60 个字符已经足够表达一件事。
    'header-max-length': [2, 'always', 60],
    'body-max-line-length': [2, 'always', 100],
  },
};
