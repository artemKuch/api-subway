import OpenAI from "openai";
import { prisma } from "../storage/prisma";

const openai = new OpenAI();

export async function listProjects() {
  return prisma.project.findMany();
}

export async function loadProject(projectId: string) {
  return prisma.project.findUnique({ where: { id: projectId } });
}

export async function createProject(input: unknown) {
  await openai.responses.create({ model: "gpt-5", input: "classify project" });
  return prisma.project.create({ data: input });
}
