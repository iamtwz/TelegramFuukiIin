// Keep messages independent of the DOM so additional locales need no UI changes.
export const locales = {
  en: { label: 'English', turnstile: 'en' },
  'zh-Hans': { label: '简体中文', turnstile: 'zh-cn' },
  'zh-Hant': { label: '繁體中文', turnstile: 'zh-tw' }
};
export const messages = {
  en: {
    brand: 'TelegramFuukiIin Verify',
    title: 'Human verification', language: 'Language', retry: 'Try again', close: 'Return to Telegram', preview: 'Preview',
    loading: 'Loading verification…', ready: 'Complete the check below to continue.',
    captcha_unavailable: 'The verification could not load; please try again.',
    verifying: 'Verifying…', submitting: 'Submitting verification…', sent: 'Submitted — check Telegram for the result.',
    open_in_telegram: 'Open this page using the verification button in Telegram.',
    invalid_link: 'This link has expired; please get a new link in Telegram.',
    turnstile_rejected: 'Verification failed; please try again.', try_later: 'Please wait a minute and try again.',
    unavailable: 'Unable to connect; please try again.', expired: 'Verification expired; please try again.',
    return_failed: 'Reopen the verification button in Telegram to continue.', preview_passed: 'Verification complete.'
  },
  'zh-Hans': {
    brand: 'Telegram 风纪委员 · 人机验证',
    title: '人机验证', language: '语言', retry: '重试', close: '返回 Telegram', preview: '预览',
    loading: '正在加载验证…', ready: '请完成下方验证。',
    captcha_unavailable: '验证码加载失败，请重试。',
    verifying: '正在验证…', submitting: '正在提交验证…', sent: '已提交，请返回 Telegram 查看结果。',
    open_in_telegram: '请从 Telegram 中的验证按钮打开。',
    invalid_link: '验证链接已失效，请在 Telegram 获取新链接。',
    turnstile_rejected: '验证未通过，请重试。', try_later: '请稍等一分钟后重试。',
    unavailable: '暂时无法连接，请重试。', expired: '验证码已过期，请重试。',
    return_failed: '请重新打开 Telegram 中的验证按钮。', preview_passed: '验证通过。'
  },
  'zh-Hant': {
    brand: 'Telegram 風紀委員 · 人機驗證',
    title: '人機驗證', language: '語言', retry: '重試', close: '返回 Telegram', preview: '預覽',
    loading: '正在載入驗證…', ready: '請完成下方驗證。',
    captcha_unavailable: '驗證碼載入失敗，請重試。',
    verifying: '正在驗證…', submitting: '正在提交驗證…', sent: '已提交，請返回 Telegram 查看結果。',
    open_in_telegram: '請從 Telegram 中的驗證按鈕開啟。',
    invalid_link: '驗證連結已失效，請在 Telegram 取得新連結。',
    turnstile_rejected: '驗證未通過，請重試。', try_later: '請稍等一分鐘後重試。',
    unavailable: '暫時無法連線，請重試。', expired: '驗證碼已過期，請重試。',
    return_failed: '請重新開啟 Telegram 中的驗證按鈕。', preview_passed: '驗證通過。'
  }
};
export function normalizeLocale(value) {
  if (typeof value !== 'string') return undefined;
  const tag = value.toLowerCase().replaceAll('_', '-');
  if (/^en(?:-|$)/.test(tag)) return 'en';
  if (!/^zh(?:-|$)/.test(tag)) return undefined;
  const parts = tag.split('-');
  if (parts.includes('hans')) return 'zh-Hans';
  return parts.some(p => ['hant', 'tw', 'hk', 'mo'].includes(p)) ? 'zh-Hant' : 'zh-Hans';
}
export function resolveLocale(...candidates) {
  for (const candidate of candidates.flat()) {
    const locale = normalizeLocale(candidate);
    if (locale) return locale;
  }
  return 'en';
}
export function translate(locale, key) {
  const catalog = messages[normalizeLocale(locale) || 'en'];
  return catalog[key] ?? messages.en[key] ?? messages.en.unavailable;
}
