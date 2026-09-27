import { test } from 'node:test';
import assert from 'node:assert/strict';
import { locales, messages, normalizeLocale, resolveLocale, translate } from '../public/i18n.js';

test('locale precedence and missing Telegram language fall back predictably', () => {
  assert.equal(resolveLocale('en-GB', 'zh-CN', 'zh-TW'), 'en');
  assert.equal(resolveLocale(null, 'zh-Hant', 'en-US'), 'zh-Hant');
  assert.equal(resolveLocale(null, null, undefined, ['fr-FR', 'zh-SG']), 'zh-Hans');
  assert.equal(resolveLocale('ar', 'de-DE'), 'en');
});
test('Chinese scripts and regional variants select the correct translation', () => {
  for (const tag of ['zh', 'zh-CN', 'zh-SG', 'zh-Hans', 'zh-Hans-HK']) assert.equal(normalizeLocale(tag), 'zh-Hans');
  for (const tag of ['zh-TW', 'zh-HK', 'zh-MO', 'zh-Hant', 'zh_Hant_TW']) assert.equal(normalizeLocale(tag), 'zh-Hant');
  assert.equal(normalizeLocale('zhuang'), undefined);
  assert.equal(normalizeLocale({ language: 'en' }), undefined);
});
test('every supported locale translates errors, controls and widget language', () => {
  for (const locale of Object.keys(locales)) {
    assert.deepEqual(Object.keys(messages[locale]).sort(), Object.keys(messages.en).sort());
    for (const key of Object.keys(messages.en)) assert.ok(typeof translate(locale, key) === 'string' && translate(locale, key).length > 0);
  }
  assert.equal(locales['zh-Hans'].turnstile, 'zh-cn');
  assert.equal(locales['zh-Hant'].turnstile, 'zh-tw');
  assert.equal(translate('unknown', 'ready'), messages.en.ready);
  assert.equal(translate('en', 'missing'), messages.en.unavailable);
});
