import { z } from 'zod';

export const UserInput = z.object({
  name: z.string().min(2).max(80),
  email: z.string().email(),
  role: z.enum(['admin', 'member']),
});

export const UserOutput = z.object({
  id: z.string().uuid(),
  name: z.string(),
  email: z.string().email(),
});
