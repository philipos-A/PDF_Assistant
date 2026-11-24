# PDF Assistant

A reading companion: a Reader opens one Document and asks Questions about it, and each Answer is grounded in that Document and in where the Reader is in it.

## Documents

**Document**:
One uploaded PDF together with its metadata (filename, Page count, size). Identified by a Document ID.
_Avoid_: Paper, file, DocInfo

**Document ID**:
The opaque identifier the backend assigns to a Document on upload.
_Avoid_: doc key, file id

**Library**:
Every Document the Reader has uploaded, with its Conversation, most recently active first.
_Avoid_: History, recent files

**Page**:
A single 1-based page of a Document, numbered as the PDF numbers it (not as printed in the paper).
_Avoid_: Slide, sheet

## Reading

**Reader**:
The person reading a Document and asking about it.
_Avoid_: User (when the reading role is meant)

**Current Page**:
The Page the Reader is mainly looking at when a Question is asked.
_Avoid_: Active page, focused page

**Visible Pages**:
Every Page at least partly on screen when a Question is asked.

**Selection**:
Text the Reader has highlighted in the Document and attached to the next Question.
_Avoid_: Highlight, quote, snippet

**Reader Context**:
The Current Page, Visible Pages and Selection sent with a Question, so the Answer knows where the Reader is.
_Avoid_: Position, viewport state

## Conversation

**Question**:
What the Reader asks, typed or spoken. A Question with only a Selection means "explain this".
_Avoid_: Prompt, query

**Answer**:
Claude's streamed reply to a Question.
_Avoid_: Response, completion

**Turn**:
One Question or one Answer in the Conversation, with the Reader Context it was asked from.
_Avoid_: HistoryTurn, ChatMessage, message

**Conversation**:
The ordered Turns about one Document. The backend saves each Question with its Answer once the Answer is done, and sends the saved Turns back to Claude with each new Question.
_Avoid_: Chat, thread, session

**Page Reference**:
A mention of a Page in an Answer ("p. 4", "pp. 5–6", "page 12") that the Reader can follow to that Page. Claude writes these as text; compare Citation.
_Avoid_: page link

**Citation**:
A passage of the Document that the API reports an Answer relies on, with the Pages it is on. Numbered within its Answer and shown as a chip; following one highlights the passage.
_Avoid_: Source, quote, reference

**Marked Passage**:
A passage of the Document that a Selection or a Citation in the Conversation points to. It stays marked in the viewer, and clicking it shows the Answers about it.
_Avoid_: Annotation, note, saved highlight

**Fallback**:
An Answer produced by the backup model because the configured model was unavailable.
