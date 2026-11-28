import type { AgentInteraction } from "./types";
import { readQuestionAnswer } from "./interactions";

export interface QuestionnaireView { questionId: string; notesOpen: string[] }
const questionForm=(root:HTMLElement):HTMLFormElement|null=>root.querySelector<HTMLFormElement>("#interaction-form:has([data-question-id])");
function questionData(form:HTMLFormElement):FormData{
  const data=new FormData(form);
  for(const field of form.querySelectorAll<HTMLInputElement|HTMLTextAreaElement>("input:disabled,textarea:disabled")){
    if(field instanceof HTMLInputElement&&["radio","checkbox"].includes(field.type)&&!field.checked)continue;
    data.append(field.name,field.value);
  }
  return data;
}
export function questionnaireView(root:HTMLElement):QuestionnaireView|undefined{
  const form=questionForm(root);
  return form?{questionId:form.dataset.questionId??"",notesOpen:[...form.querySelectorAll<HTMLDetailsElement>("[data-question-other][open]")].map(detail=>detail.dataset.questionOther!)}:undefined;
}
export function questionPage(root:HTMLElement,item:AgentInteraction):number{
  return Math.max(0,(item.data.questions??[]).findIndex(question=>question.id===questionForm(root)?.dataset.questionId));
}
export function unansweredQuestion(root:HTMLElement,item:AgentInteraction,index?:number):number{
  const form=questionForm(root);if(!form)return -1;
  const data=questionData(form);
  return (item.data.questions??[]).findIndex((question,position)=>(index===undefined||position===index)&&!readQuestionAnswer(question,position,data).length);
}
export function syncQuestionnaire(root:HTMLElement,item:AgentInteraction,view?:QuestionnaireView):void{
  const form=questionForm(root),questions=item.data.questions??[];
  if(!form||!questions.length)return;
  if(view)form.dataset.questionId=view.questionId;
  const page=questionPage(root,item);form.dataset.questionId=questions[page].id;
  for(const section of form.querySelectorAll<HTMLElement>("[data-question-id]"))section.hidden=section.dataset.questionId!==questions[page].id;
  for(const detail of form.querySelectorAll<HTMLDetailsElement>("[data-question-other]")){
    if(view)detail.open=view.notesOpen.includes(detail.dataset.questionOther!);
    const filled=Boolean(detail.querySelector<HTMLInputElement|HTMLTextAreaElement>("input,textarea")?.value.trim());
    detail.querySelector<HTMLElement>("[data-notes-state]")!.textContent=filled?" · 已填写":"";
  }
  const data=questionData(form),answered=questions.filter((question,index)=>readQuestionAnswer(question,index,data).length).length;
  for(const progress of root.querySelectorAll<HTMLElement>("[data-question-progress]")){
    const text="问题 "+(page+1)+"/"+questions.length+" · 已答 "+answered;
    if(progress.textContent!==text)progress.textContent=text;
  }
  form.querySelector<HTMLElement>('[data-question-nav="-1"]')!.hidden=page===0;
  form.querySelector<HTMLElement>('[data-question-nav="1"]')!.hidden=page===questions.length-1;
  form.querySelector<HTMLElement>('button[type="submit"]')!.hidden=page!==questions.length-1;
  for(const field of form.querySelectorAll<HTMLTextAreaElement>("textarea")){
    if(!field.getClientRects().length)continue;
    field.style.height="auto";field.style.height=Math.min(120,Math.max(44,field.scrollHeight+2))+"px";
  }
}
export function showQuestion(root:HTMLElement,item:AgentInteraction,index:number):void{
  const form=questionForm(root),question=item.data.questions?.[index];if(!form||!question)return;
  form.dataset.questionId=question.id;syncQuestionnaire(root,item);
  const scroll=form.querySelector(".interaction-scroll");if(scroll)scroll.scrollTop=0;
  form.querySelector<HTMLElement>("fieldset:not([hidden]) legend")?.focus({preventScroll:true});
}
