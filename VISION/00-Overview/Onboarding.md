# Onboarding and First-Run Experience

## Installation

### Primary Method
```bash
curl --proto '=https' --tlsv1.2 -sSf https://install.agent.dev | sh
```

This installs:
- The `agent` binary to `~/.local/bin/`
- A systemd user service file (Linux) or launchd plist (macOS)
- Default config scaffolding to `~/.config/agent/`

### Alternative: Cargo
```bash
cargo install agent
```

### Alternative: Docker
```bash
docker run -v ~/.config/agent:/config -v ~/.local/share/agent:/data agent:latest
```

## First-Run Wizard

After installation, running `agent start` for the first time launches the wizard.

### Step 1: Welcome
```
Welcome. I am your personal agent. I learn from your life, 
connect to your services, and become more useful over time.

I will start by observing. I will not act without your permission.
As I learn your patterns, I will offer to help. You decide what I can do.

Let's get connected.
```

### Step 2: Connect Services

The wizard presents starting services. Some require auth, some work immediately.

**Built-in Services (No Auth Required):**
- **Web Scraping** — I can read and extract information from websites
- **Local Files** — I can search and read files on your computer
- **System Info** — I can check your system status, time, weather (via API)
- **Calculator** — I can do math and date calculations

**Services Requiring Auth:**
- Email (Gmail, IMAP)
- Calendar (Google, Apple, CalDAV)
- Photos (Google Photos, local folders)
- GitHub
- Spotify
- Home Assistant
- Signal

The wizard shows one service at a time:
```
📧 Connect Email
I can learn from your emails: trips, bookings, events, receipts.

[Connect Gmail] [Connect IMAP] [Skip for now]
```

For each connected service, the wizard explains what the agent will learn and what it will not do.

### Step 3: Historic Ingestion

If the user connects a service, the wizard asks about historical data:

```
📜 Learn from History?

I can look back through your emails and calendar to learn your patterns:
- Places you have traveled
- How you prefer to travel
- Your routines and preferences

I will NOT download and store every email. I will extract patterns and then discard the raw data.

[Learn from last 6 months] [Learn from last year] [Start from today only]
```

**What historic ingestion extracts:**
- Travel history (destinations, frequency, airlines, class preferences)
- Recurring events and routines
- Contact relationships
- Shopping patterns (if receipts found)
- Hobby and interest signals

**What historic ingestion does NOT extract:**
- Full email contents (patterns only)
- Sensitive medical or financial details (unless explicitly permitted later)
- Private conversations verbatim

### Step 4: LLM Configuration

```
🧠 Configure Language Model

I use an OpenAI-compatible API for reasoning and natural language.
You can use OpenAI, Anthropic, a local model, or any compatible endpoint.

Endpoint: [https://api.openai.com/v1________]
API Key:  [sk-________________________________]
Model:    [gpt-5______________________________]

[Use local model (Ollama)] [Test connection] [Skip and configure later]
```

### Step 5: Personality Selection

```
🎭 Choose Personality

How would you like me to communicate?

[Transparent] — I show my work and explain my reasoning. (Default)
[Concise] — Brief and to the point. No fluff.
[Warm] — Conversational and personable.
[Custom] — Write your own system prompt.

[Select] [Preview each]
```

### Step 6: Privacy and Permissions

```
🔒 Privacy Settings

Before I start learning, confirm your boundaries:

[x] I can ingest all data by default (you can restrict later)
[ ] Ask before storing sensitive topics (medical, financial)
[ ] Do not learn about: [medical] [financial] [relationships] [work]

I am local-first. Your data stays on your device unless you configure otherwise.

[Continue]
```

### Step 7: First Value Moment

The wizard ends with an immediate demonstration:

```
✅ You are all set.

Try asking me something:

> "What does my week look like?"
> "Summarize the last email about my trip."
> "When did I last visit Japan?"

Or just let me observe. I will notify you when I have something useful.

Run `agent chat` to start a conversation.
Run `agent help` to see all commands.
```

## Gradual Adoption

The user does not adopt the agent in a single moment. It happens gradually:

### Week 1: Observation
- Agent is connected but mostly silent
- User may ask occasional questions
- Agent is learning patterns in the background
- No proactive suggestions yet

### Week 2–4: First Offers
- Agent detects first helpful patterns
- Offers are infrequent and low-stakes
- User accepts or rejects, teaching the agent

### Month 2–3: Permission Grants
- Agent offers category-level permissions
- User starts trusting it with specific tasks
- Proactive suggestions become useful

### Month 6+: Invisible Utility
- Agent handles routine tasks automatically
- User forgets it is there until it saves them
- It feels like an extension of memory, not a tool

## Example First Value Moments

### The Birthday Gift
```
🔔 Price Drop Alert

In March, your sister mentioned her son's birthday is June 15th 
and he wants an Xbox Series X.

I have been monitoring prices. It is now £349 at Currys — 
£50 below the 6-month average and the lowest since January.

Want me to keep monitoring or remind you closer to the date?

[Remind me June 1st] [Keep monitoring] [Noted, thanks]
```

### The Scheduling Question
```
> Do I have time to meet Alice for coffee on Saturday?

You have a free block from 2 PM to 5 PM on Saturday.
Alice is usually free on Saturday afternoons based on her shared calendar.
You are near her neighborhood that day (photo from last Saturday at the same park).

However, your mum's birthday dinner is at 6 PM, so you will need to leave by 5.
A 2 PM coffee gives you plenty of time.

Want me to suggest a time to Alice?
```

### The Travel Reminder
```
🔔 Flight Tomorrow

Your flight to Lisbon is tomorrow at 9 AM.
You usually leave for the airport 2 hours before.
It is a short-haul flight, so just a carry-on is fine.
The weather in Lisbon is 24°C and sunny.

No action needed — just a heads up.
```

## Retention and Engagement

The agent does not nag. It respects the user's attention:
- No daily "check in" messages
- No marketing-style notifications
- Proactivity only when genuinely useful
- User can always say "leave me alone for 3 days"

The goal is that the user forgets the agent exists until it saves them.
