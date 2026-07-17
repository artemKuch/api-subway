import { loadUser } from '../../../../services/users';
import Stripe from 'stripe';

export const GET = async () => Response.json(await loadUser());
export const PATCH = async () => Response.json({ ok: true });
