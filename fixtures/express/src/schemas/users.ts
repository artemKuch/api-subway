import { z } from 'zod';

export const UserInput = z.object({
  name: z.string().min(2),
  role: z.enum(['admin', 'member']),
});

export const UserOutput = z.object({
  id: z.string().uuid(),
  name: z.string(),
  role: z.enum(['admin', 'member']),
});
