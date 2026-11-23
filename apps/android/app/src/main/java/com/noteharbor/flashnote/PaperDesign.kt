package com.noteharbor.flashnote

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.*
import androidx.compose.ui.graphics.*
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.*
import androidx.compose.ui.text.font.*
import androidx.compose.ui.text.style.*
import androidx.compose.ui.unit.*

// Match apps/desktop/src/ui.rs exactly. No dynamic/system color substitution.
internal val Paper=Color(0xFFF7F3EA)
internal val Ink=Color(0xFF245356)
internal val Edge=Color(0xFFD9D5CB)
internal val CardPaper=Color(0xFFFBF8F1)
internal val Muted=Color(0xFF68645E)
internal val TextInk=Color(0xFF262723)
internal val Quiet=Color(0xFFF0ECE3)

@Composable fun PaperTheme(content: @Composable () -> Unit) {
 MaterialTheme(colorScheme=lightColorScheme(primary=Ink,onPrimary=CardPaper,primaryContainer=Color(0xFFDDE7DF),onPrimaryContainer=Ink,
  secondary=Ink,onSecondary=CardPaper,secondaryContainer=Quiet,onSecondaryContainer=TextInk,
  tertiary=Ink,background=Paper,onBackground=TextInk,surface=CardPaper,onSurface=TextInk,
  surfaceVariant=Quiet,onSurfaceVariant=Muted,surfaceTint=Color.Transparent,outline=Edge,outlineVariant=Edge),
  typography=Typography(bodyLarge=TextStyle(fontFamily=FontFamily.SansSerif,fontSize=16.sp,lineHeight=25.sp),
   bodyMedium=TextStyle(fontFamily=FontFamily.SansSerif,fontSize=14.sp,lineHeight=22.sp),
   titleLarge=TextStyle(fontFamily=FontFamily.SansSerif,fontSize=22.sp,fontWeight=FontWeight.SemiBold),
   labelLarge=TextStyle(fontFamily=FontFamily.SansSerif,fontSize=14.sp,fontWeight=FontWeight.Medium)),content=content)
}

@Composable internal fun LineIcon(name: String,description: String?=null,modifier: Modifier=Modifier.size(20.dp),color: Color=Ink) {
 val resource=when(name) {
  "bold" -> R.drawable.ic_bold
  "check" -> R.drawable.ic_check
  "close" -> R.drawable.ic_close
  "down" -> R.drawable.ic_down
  "file" -> R.drawable.ic_file
  "grid" -> R.drawable.ic_grid
  "history" -> R.drawable.ic_history
  "image" -> R.drawable.ic_image
  "italic" -> R.drawable.ic_italic
  "link" -> R.drawable.ic_link
  "list" -> R.drawable.ic_list
  "maximize" -> R.drawable.ic_maximize
  "mic" -> R.drawable.ic_mic
  "minus" -> R.drawable.ic_minus
  "more" -> R.drawable.ic_more
  "pause" -> R.drawable.ic_pause
  "pencil" -> R.drawable.ic_pencil
  "pin" -> R.drawable.ic_pin
  "play" -> R.drawable.ic_play
  "plus" -> R.drawable.ic_plus
  "right" -> R.drawable.ic_right
  "search" -> R.drawable.ic_search
  "settings" -> R.drawable.ic_settings
  "stop" -> R.drawable.ic_stop
  "tick" -> R.drawable.ic_tick
  "trash" -> R.drawable.ic_trash
  "pin-filled" -> R.drawable.ic_pin_filled
  "book" -> R.drawable.ic_book
  "home" -> R.drawable.ic_home
  "heart" -> R.drawable.ic_heart
  "wallet" -> R.drawable.ic_wallet
  "plane" -> R.drawable.ic_plane
  "calendar" -> R.drawable.ic_calendar
  "star" -> R.drawable.ic_star
  "folder" -> R.drawable.ic_folder
  "inbox" -> R.drawable.ic_inbox
  "briefcase" -> R.drawable.ic_briefcase
  "bulb" -> R.drawable.ic_bulb
  "coffee" -> R.drawable.ic_coffee
  "dot" -> R.drawable.ic_dot
  "ordered" -> R.drawable.ic_ordered
  "quote" -> R.drawable.ic_quote
  "code" -> R.drawable.ic_code
  "brand" -> R.drawable.ic_brand
  "user" -> R.drawable.ic_user
  "cloud-check" -> R.drawable.ic_cloud_check
  "cloud" -> R.drawable.ic_cloud
  "back" -> R.drawable.ic_back
  "note" -> R.drawable.ic_note
  else -> R.drawable.ic_folder
 }
 Icon(painterResource(resource),description,modifier,tint=color)
}
@Composable internal fun QuietAction(icon: String,label: String,enabled: Boolean=true,onClick: () -> Unit) {
 IconButton(onClick=onClick,enabled=enabled,modifier=Modifier.size(44.dp)) {LineIcon(icon,label,color=if(enabled) Ink else Muted.copy(alpha=.45f))}
}
@Composable internal fun PaperSearch(value: String,onChange: (String)->Unit) {
 Surface(shape=RoundedCornerShape(13.dp),color=Quiet.copy(alpha=.45f),border=BorderStroke(1.dp,Edge),modifier=Modifier.fillMaxWidth().padding(horizontal=20.dp)) {
  Row(Modifier.heightIn(min=40.dp).padding(horizontal=14.dp,vertical=8.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {
   LineIcon("search",modifier=Modifier.size(18.dp),color=Muted)
   BasicTextField(value,onChange,singleLine=true,textStyle=TextStyle(fontSize=14.sp,color=TextInk),cursorBrush=SolidColor(Ink),modifier=Modifier.weight(1f),decorationBox={inner -> if(value.isEmpty()) Text("搜索便签",fontSize=14.sp,color=Muted);inner()})
  }
 }
}
@Composable internal fun PaperChip(label: String,selected: Boolean,onClick: () -> Unit) {
 Box(Modifier.heightIn(min=44.dp).clickable(onClick=onClick),contentAlignment=Alignment.Center) {
  Surface(shape=RoundedCornerShape(18.dp),color=if(selected) Ink else Color.Transparent,border=if(selected) null else BorderStroke(1.dp,Edge)) {
   Text(label,modifier=Modifier.padding(horizontal=20.dp,vertical=6.dp),fontSize=14.sp,lineHeight=20.sp,color=if(selected) CardPaper else TextInk,fontWeight=if(selected) FontWeight.Medium else FontWeight.Normal)
  }
 }
}
@Composable internal fun IndexPaper(color: Color,onClick: () -> Unit,content: @Composable () -> Unit) {
 Box(Modifier.fillMaxWidth().padding(top=5.dp)) {
  Surface(onClick=onClick,modifier=Modifier.fillMaxWidth(),shape=RoundedCornerShape(8.dp),color=CardPaper,border=BorderStroke(1.dp,Edge),shadowElevation=0.4.dp) {content()}
  Canvas(Modifier.offset(y=(-4).dp).size(43.dp,9.dp)) {
   val p=Path().apply {moveTo(0f,size.height);quadraticTo(0f,0f,size.height,0f);lineTo(size.width,0f);quadraticTo(size.width-3.dp.toPx(),size.height-1.dp.toPx(),size.width-9.dp.toPx(),size.height-1.dp.toPx());lineTo(7.dp.toPx(),size.height-1.dp.toPx());close()}
   drawPath(p,color)
  }
 }
}

// Compact, styled previews use the actual note content; no emoji controls or raw Markdown tokens.
@Composable internal fun NoteExcerpt(source: String,limit: Int=4,modifier: Modifier=Modifier) {
 val rows=source.replace(Regex("\\[素材\\]\\(attachment:[^)]*\\)"),"").lines().filter {it.isNotBlank()}.take(limit)
 Column(modifier,verticalArrangement=Arrangement.spacedBy(4.dp)) {
  rows.forEach {line ->
   val task=Regex("^\\s*[-*] \\[([ xX])\\] (.*)$").find(line)
   if(task!=null) Row(verticalAlignment=Alignment.Top,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
    val checked=task.groupValues[1]!=" "
    Box(Modifier.padding(top=3.dp).size(14.dp).background(if(checked) Ink else Color.Transparent,RoundedCornerShape(2.dp)).border(1.dp,if(checked) Ink else Muted,RoundedCornerShape(2.dp)),contentAlignment=Alignment.Center) {if(checked) LineIcon("tick",modifier=Modifier.size(12.dp),color=CardPaper)}
    Text(task.groupValues[2].replace("**",""),fontSize=14.sp,lineHeight=21.sp,color=if(checked) Muted else TextInk,textDecoration=if(checked) TextDecoration.LineThrough else null,maxLines=2,overflow=TextOverflow.Ellipsis)
   } else {
    val heading=line.startsWith("#")
    Text(line.replace(Regex("^#{1,6}\\s+"),"").replace("**","").replace(Regex("^[-*] "),"• "),fontSize=14.sp,lineHeight=21.sp,fontWeight=if(heading) FontWeight.SemiBold else FontWeight.Normal,color=TextInk,maxLines=3,overflow=TextOverflow.Ellipsis)
   }
  }
 }
}
