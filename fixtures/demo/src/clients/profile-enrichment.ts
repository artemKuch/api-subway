import OpenAI from 'openai';

const client = new OpenAI();

export async function enrichProfile(user: unknown) {
  await client.responses.create({ model: 'fixture', input: String(user) });
  return user;
}
