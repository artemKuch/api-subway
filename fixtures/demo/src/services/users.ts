import { enrichProfile } from '../clients/profile-enrichment';
import { sendWelcome } from '../integrations/notifications';
import { findUser, findUsers, insertUser } from '../repositories/users';

export async function listUsers() {
  return findUsers();
}

export async function createUser(input: unknown) {
  const user = await insertUser(input);
  await sendWelcome(user);
  return user;
}

export async function getUser(id: string) {
  const user = await findUser(id);
  return enrichProfile(user);
}
