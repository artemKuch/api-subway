import { prisma } from '../../../lib/prisma';
import { UserInput, UserOutput } from '../../../schemas/users';

export async function GET() {
  return Response.json(await prisma.user.findMany());
}

export async function POST(request: Request) {
  const input = UserInput.parse(await request.json());
  const user = await prisma.user.create({ data: input });
  return Response.json(UserOutput.parse(user), { status: 201 });
}
