package com.noteharbor.flashnote

import android.app.Application
import androidx.compose.runtime.*
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.util.concurrent.Executors

data class Attachment(val asset: String, val name: String) {
 val isImage: Boolean get()=name.substringAfterLast('.').lowercase() in listOf("jpg","jpeg","png","webp","gif")
 val isAudio: Boolean get()=name.substringAfterLast('.').lowercase() in listOf("wav","mp3","m4a","ogg","aac")
}
data class Group(val id: String, val name: String, val color: Long = 0x245356)
data class Note(val id: String, val revision: String, val title: String, val group: String,
 val source: String, val deleted: Boolean, val created: Long, val conflict: String?, val attachments: List<Attachment> = emptyList())

class NotesModel(app: Application, preview: Boolean=false) : AndroidViewModel(app) {
 private val executor = Executors.newSingleThreadExecutor()
 private val worker = executor.asCoroutineDispatcher()
 private val root = app.filesDir.resolve(if(BuildConfig.DEBUG && preview) "design-preview-v2" else "vault").absolutePath
 val assetRoot get()=java.io.File(root,"assets")
 var notes by mutableStateOf(emptyList<Note>()); private set
 var groups by mutableStateOf(emptyList<Group>()); private set
 var account by mutableStateOf<String?>(null); private set
 var busy by mutableStateOf(false); private set
 var syncing by mutableStateOf(false); private set
 var synced by mutableStateOf(false); private set
 var syncFailed by mutableStateOf(false); private set
 var error by mutableStateOf<String?>(null)
 var lastSavedNote: Note? = null; private set
 var message by mutableStateOf<String?>(null)
 init { send(JSONObject().put("op","list"),prepare={if(BuildConfig.DEBUG && preview) DesignFixtures.seed(app,root)}) }
 fun send(request: JSONObject, prepare: (() -> Unit)? = null, done: () -> Unit = {}) {
  if (busy) return
  busy = true
  syncing = request.optString("op")=="sync"
  viewModelScope.launch {
   try {
    val response = withContext(worker) { prepare?.invoke(); JSONObject(Native.request(root,request.toString())) }
    check(response.getBoolean("ok")) { response.optString("error","操作失败") }
    val data = response.getJSONObject("data")
    val ns = data.getJSONArray("notes")
    val changed = (0 until ns.length()).map { i ->
     val row=ns.getJSONObject(i); val n=row.getJSONObject("note")
     Note(n.getString("id"),n.getString("revision"),n.getString("title"),n.getString("group_id"),
      row.getString("source"), !n.isNull("deleted_at"),n.getLong("created_at"),
      if(n.isNull("conflict_of") || n.optBoolean("conflict_resolved")) null else n.getString("conflict_of"),
      n.getJSONArray("blocks").let { blocks -> (0 until blocks.length()).map { blocks.getJSONObject(it) }
       .filter { it.has("asset") && it.getString("asset").isNotEmpty() }
       .map { Attachment(it.getString("asset"),it.optString("name",if(it.optString("kind")=="audio") "录音.wav" else "附件")) } })
    }
    if (data.getBoolean("replace_notes")) {
     notes = changed.sortedByDescending { it.created }
    } else if (changed.isNotEmpty()) {
     val changesById = changed.associateBy { it.id }
     val currentIds = notes.mapTo(HashSet()) { it.id }
     notes = (notes.map { changesById[it.id] ?: it } + changed.filter { it.id !in currentIds })
      .sortedByDescending { it.created }
    }
    if (!data.isNull("saved_id")) lastSavedNote = changed.find { it.id == data.getString("saved_id") }
    val gs=data.getJSONArray("groups")
    groups=(0 until gs.length()).map { gs.getJSONObject(it) }.filter { !it.optBoolean("deleted") }.map { Group(it.getString("id"),it.getString("name"),it.optLong("color",0x245356)) }
    account=if(data.isNull("account")) null else data.getString("account")
    when(request.optString("op")) {
     "sync" -> {
      val syncError = if (data.isNull("sync_error")) null else data.getString("sync_error")
      synced = syncError == null
      syncFailed = syncError != null
      error = syncError
     }
     "login", "logout", "save", "attach", "group", "delete", "restore", "resolve" -> synced=false
    }
    done()
   } catch(e: Exception) { if(syncing) syncFailed=true;error=e.message ?: "操作失败，草稿仍保留" }
   finally { busy=false;syncing=false }
  }
 }
 fun save(note: Note?, title: String, source: String, group: String, done: () -> Unit) = send(JSONObject()
  .put("op","save").put("id",note?.id ?: "").put("revision",note?.revision ?: "")
  .put("title",title).put("source",source).put("group",group),done=done)
 fun change(note: Note, op: String, done: () -> Unit = {}) = send(JSONObject().put("op",op).put("id",note.id).put("revision",note.revision),done=done)
 fun attach(uri: android.net.Uri, note: Note) {
  val app=getApplication<Application>(); val token=java.util.UUID.randomUUID().toString()
  var name="附件"
  app.contentResolver.query(uri,null,null,null,null)?.use { cursor ->
   val i=cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
   if(cursor.moveToFirst() && i>=0) name=cursor.getString(i)
  }
  send(JSONObject().put("op","attach").put("id",note.id).put("revision",note.revision).put("token",token).put("name",name),prepare={
   val directory=java.io.File(root,"imports").apply {mkdirs()}; val file=java.io.File(directory,token)
   try {
    app.contentResolver.openInputStream(uri)!!.use { input -> file.outputStream().use { output ->
     val buffer=ByteArray(8192); var total=0L
     while(true) {val count=input.read(buffer);if(count<0) break;total+=count;check(total<=32L*1024*1024) {"附件超过 32MB"};output.write(buffer,0,count)}
    } }
   } catch(e: Exception) {file.delete();throw e}
  })
 }
 fun openAttachment(item: Attachment) {
  if(busy) return
  busy=true
  viewModelScope.launch {
   try {
    val data=withContext(worker) {JSONObject(Native.request(root,JSONObject().put("op","asset").put("asset",item.asset).put("name",item.name).toString()))}
    check(data.getBoolean("ok")) {data.optString("error")}
    val app=getApplication<Application>()
    val uri=androidx.core.content.FileProvider.getUriForFile(app,app.packageName+".files",java.io.File(data.getJSONObject("data").getString("file")))
    val ext=item.name.substringAfterLast('.',"").lowercase()
    val mime=android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
    val intent=android.content.Intent(android.content.Intent.ACTION_VIEW).setDataAndType(uri,mime).addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
    app.startActivity(intent)
   } catch(e: Exception) {error=e.message ?: "无法打开附件"}
   finally {busy=false}
  }
 }
 override fun onCleared() { worker.close(); executor.shutdown(); super.onCleared() }
}
