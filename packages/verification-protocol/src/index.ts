// Parameters are public routing hints, never proof of identity or completion.
export interface Launch { session: string; chat: string; sitekey: string }
export interface Submission { v: 2; session: string; chat: string; token: string }
export function readLaunch(search: string): Launch {
  const params = new URLSearchParams(search);
  for (const key of ['session', 'chat', 'sitekey']) {
    if (params.getAll(key).length !== 1) throw new Error('invalid_link');
  }
  const session = params.get('session')!;
  const chat = params.get('chat')!;
  const sitekey = params.get('sitekey')!;
  if (!/^[a-f0-9]{32}$/.test(session) || !/^-[1-9][0-9]*$/.test(chat)
    || !Number.isSafeInteger(Number(chat)) || !/^[a-zA-Z0-9_-]{1,100}$/.test(sitekey)) throw new Error('invalid_link');
  return { session, chat, sitekey };
}
export function encodeSubmission(launch: Launch, token: string): string {
  readLaunch(new URLSearchParams({ ...launch }).toString());
  if (typeof token !== 'string' || !/^[\x21-\x7e]{1,2048}$/.test(token)) throw new Error('invalid_token');
  const data: Submission = { v: 2, session: launch.session, chat: launch.chat, token };
  const raw = JSON.stringify(data);
  if (new TextEncoder().encode(raw).length > 4096) throw new Error('invalid_token');
  return raw;
}
