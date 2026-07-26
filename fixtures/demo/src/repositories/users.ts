import { PrismaClient } from '@prisma/client';

const database = new PrismaClient();

export async function findUsers() {
  return database.user.findMany();
}

export async function insertUser(input: unknown) {
  return database.user.create({ data: input });
}

export async function findUser(id: string) {
  return database.user.findUnique({ where: { id } });
}
